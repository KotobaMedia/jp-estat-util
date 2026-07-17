use crate::unzip;
use anyhow::{Context, Result, anyhow, bail};
use csv::{ReaderBuilder, StringRecord};
use encoding_rs::SHIFT_JIS;
use encoding_rs_io::DecodeReaderBytesBuilder;
use jismesh::codes::JAPAN_LV1;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};
use tokio::io::AsyncWriteExt as _;

const DATA_COLUMN_START: usize = 4;

#[derive(Debug, Deserialize, Clone)]
struct MeshStatsConfig {
    mesh_stats: Vec<MeshStats>,
}

#[derive(Debug, Deserialize, Clone)]
struct MeshStats {
    name: String,
    year: u16,
    meshlevel: u8,
    stats_id: String,
    datum: u16,
}

#[derive(Debug)]
struct DatasetInfo {
    mesh_stats: MeshStats,
    bands: Option<Vec<String>>,
    bands_error: Option<String>,
}

lazy_static::lazy_static! {
    static ref AVAILABLE: Vec<MeshStats> = {
        let json_str = include_str!("mesh_stats.json");
        let config: MeshStatsConfig = serde_json::from_str(json_str)
            .expect("Failed to parse mesh_stats.json");
        config.mesh_stats
    };
}

fn open_shiftjis_csv(path: &Path) -> Result<csv::Reader<Box<dyn std::io::Read>>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let transcoded = DecodeReaderBytesBuilder::new()
        .encoding(Some(SHIFT_JIS))
        .build(reader);

    Ok(ReaderBuilder::new()
        .has_headers(false)
        .from_reader(Box::new(transcoded)))
}

fn normalize_headers(header1: &StringRecord, header2: &StringRecord) -> Vec<String> {
    header2
        .iter()
        .enumerate()
        .map(|(i, h2)| {
            let col = if h2.trim().is_empty() {
                header1.get(i).unwrap_or_default().to_string()
            } else {
                h2.to_string()
            };
            col.trim().replace("\u{3000}", "")
        })
        .collect()
}

fn extract_bands(csv_path: &Path) -> Result<Vec<String>> {
    let mut rdr = open_shiftjis_csv(csv_path)?;
    let header1 = rdr
        .records()
        .next()
        .transpose()?
        .ok_or(anyhow!("missing first header row"))?;
    let header2 = rdr
        .records()
        .next()
        .transpose()?
        .ok_or(anyhow!("missing second header row"))?;

    let normalized = normalize_headers(&header1, &header2);
    if normalized.len() <= DATA_COLUMN_START {
        bail!("CSV has too few columns");
    }

    Ok(normalized[DATA_COLUMN_START..].to_vec())
}

fn build_mesh_url(stats_id: &str, mesh_code: u64) -> String {
    format!(
        "https://www.e-stat.go.jp/gis/statmap-search/data?statsId={}&code={}&downloadType=2",
        stats_id, mesh_code
    )
}

async fn download_zip(client: &Client, zip_path: &Path, url: &str) -> Result<StatusCode> {
    let response = client.get(url).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Ok(status);
    }

    let content = response.bytes().await?;
    let mut file = tokio::fs::File::create(zip_path).await?;
    file.write_all(&content).await?;
    file.flush().await?;
    Ok(status)
}

async fn try_extract_txt(zip_path: &Path) -> Option<PathBuf> {
    let extracted = unzip::unzip_archive(zip_path).await.ok()?;
    unzip::find_file_with_ext(&extracted, "txt").await.ok()
}

async fn ensure_sample_csv(tmp_dir: &Path, client: &Client, stats: &MeshStats) -> Result<PathBuf> {
    for mesh in JAPAN_LV1 {
        let zip_filename = format!("{}-{}-{}.zip", stats.year, stats.stats_id, mesh);
        let zip_path = tmp_dir.join(zip_filename);
        if !zip_path.exists() {
            continue;
        }
        if let Some(txt_path) = try_extract_txt(&zip_path).await {
            return Ok(txt_path);
        }
    }

    for mesh in JAPAN_LV1.iter().copied() {
        let zip_filename = format!("{}-{}-{}.zip", stats.year, stats.stats_id, mesh);
        let zip_path = tmp_dir.join(zip_filename);
        let url = build_mesh_url(&stats.stats_id, mesh);
        let status = download_zip(client, &zip_path, &url)
            .await
            .with_context(|| format!("failed to download {}", url))?;

        if status == StatusCode::NOT_FOUND {
            continue;
        }
        if !status.is_success() {
            bail!("failed to download {} [{}]", url, status);
        }

        if let Some(txt_path) = try_extract_txt(&zip_path).await {
            return Ok(txt_path);
        }
    }

    bail!(
        "No sample CSV found for stats_id={} (survey='{}', year={}, level={})",
        stats.stats_id,
        stats.name,
        stats.year,
        stats.meshlevel
    )
}

fn join_u16(set: &BTreeSet<u16>) -> String {
    set.iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn join_u8(set: &BTreeSet<u8>) -> String {
    set.iter().map(u8::to_string).collect::<Vec<_>>().join(", ")
}

fn format_error_chain(err: &anyhow::Error) -> String {
    err.chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(": ")
}

fn print_report(datasets: &[DatasetInfo]) {
    let mut by_survey: BTreeMap<String, Vec<&DatasetInfo>> = BTreeMap::new();
    for dataset in datasets {
        by_survey
            .entry(dataset.mesh_stats.name.clone())
            .or_default()
            .push(dataset);
    }

    println!("利用可能なメッシュ統計データ");
    println!("調査種別数: {}", by_survey.len());
    println!("データセット数: {}", datasets.len());
    println!();

    for (survey, rows) in by_survey {
        let mut years = BTreeSet::new();
        let mut levels = BTreeSet::new();
        for row in &rows {
            years.insert(row.mesh_stats.year);
            levels.insert(row.mesh_stats.meshlevel);
        }

        println!("調査: {}", survey);
        println!("  年度: {}", join_u16(&years));
        println!("  レベル: {}", join_u8(&levels));
        println!("  データセット:");

        for row in rows {
            let bands_count = row.bands.as_ref().map_or(0, |bands| bands.len());
            println!(
                "    - year={} level={} stats_id={} datum={} bands={}",
                row.mesh_stats.year,
                row.mesh_stats.meshlevel,
                row.mesh_stats.stats_id,
                row.mesh_stats.datum,
                bands_count,
            );
            if let Some(bands) = row.bands.as_ref() {
                for band in bands {
                    println!("      - {}", band);
                }
            } else if let Some(reason) = row.bands_error.as_ref() {
                println!("      - (取得失敗) {}", reason);
            } else {
                println!("      - (取得失敗) unknown error");
            }
        }
        println!();
    }
}

pub async fn process_mesh_info(tmp_dir: &Path, year_filter: Option<&[u16]>) -> Result<()> {
    let mut available = AVAILABLE.clone();
    if let Some(years) = year_filter {
        let years_set: BTreeSet<u16> = years.iter().copied().collect();
        available.retain(|stats| years_set.contains(&stats.year));
    }
    available.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.year.cmp(&b.year))
            .then_with(|| a.meshlevel.cmp(&b.meshlevel))
            .then_with(|| a.stats_id.cmp(&b.stats_id))
    });
    if available.is_empty() {
        if let Some(years) = year_filter {
            let years = years
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "利用可能なメッシュ統計データは見つかりませんでした (year filter: {})",
                years
            );
        } else {
            println!("利用可能なメッシュ統計データは見つかりませんでした");
        }
        return Ok(());
    }

    let client = Client::new();
    let mut datasets = Vec::with_capacity(available.len());
    for stats in available {
        let (bands, bands_error) = match ensure_sample_csv(tmp_dir, &client, &stats)
            .await
            .with_context(|| {
                format!(
                    "when resolving sample CSV for survey='{}' year={} level={} stats_id={}",
                    stats.name, stats.year, stats.meshlevel, stats.stats_id
                )
            }) {
            Ok(sample_csv) => match extract_bands(&sample_csv)
                .with_context(|| format!("when parsing bands from {}", sample_csv.display()))
            {
                Ok(bands) => (Some(bands), None),
                Err(err) => (None, Some(format_error_chain(&err))),
            },
            Err(err) => (None, Some(format_error_chain(&err))),
        };
        datasets.push(DatasetInfo {
            mesh_stats: stats,
            bands,
            bands_error,
        });
    }

    print_report(&datasets);
    Ok(())
}
