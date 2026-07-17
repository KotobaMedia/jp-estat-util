use crate::download::{self, DownloadedItem};
use anyhow::{Context, Result, anyhow};
use csv::{ReaderBuilder, StringRecord, WriterBuilder};
use encoding_rs::SHIFT_JIS;
use encoding_rs_io::DecodeReaderBytesBuilder;
use futures::stream;
use indicatif::{ProgressBar, ProgressStyle};
use jismesh::codes::JAPAN_LV1;
use serde::Deserialize;
use std::{fs::File, io::BufReader, path::Path};
use url::Url;

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

    #[allow(dead_code)]
    datum: u16,
}

lazy_static::lazy_static! {
    static ref AVAILABLE: Vec<MeshStats> = {
        let json_str = include_str!("mesh_stats.json");
        let config: MeshStatsConfig = serde_json::from_str(json_str)
            .expect("Failed to parse mesh_stats.json");
        config.mesh_stats
    };
}

fn get_matching_mesh_stats(level: u8, year: u16, survey: &str) -> Option<&'static MeshStats> {
    AVAILABLE.iter().find(|&mesh| mesh.meshlevel == level && mesh.year == year && mesh.name == survey).map(|v| v as _)
}

pub async fn process_mesh_csv(
    tmp_dir: &Path,
    level: u8,
    year: u16,
    survey: &str,
    output: &Path,
) -> Result<()> {
    let mesh_stats = get_matching_mesh_stats(level, year, survey)
        .ok_or(anyhow!("一致する統計データが見つかりません"))?;

    let urls_with_metadata: Vec<(u64, Url)> = JAPAN_LV1
        .iter()
        .map(|mesh| {
            let url = format!(
                "https://www.e-stat.go.jp/gis/statmap-search/data?statsId={}&code={}&downloadType=2",
                mesh_stats.stats_id, mesh
            );
            (*mesh, Url::parse(&url).unwrap())
        })
        .collect();

    let mut downloaded_items: Vec<DownloadedItem<(u64, Url)>> = download::download_and_extract_all(
        stream::iter(urls_with_metadata),
        |(_mesh, url)| url.clone(),
        |(mesh, _url)| format!("{}-{}-{}.zip", mesh_stats.year, mesh_stats.stats_id, mesh),
        "txt",
        tmp_dir,
        "Downloading Mesh CSVs...",
        "Extracting Mesh CSVs...",
        10,
    )
    .await?;

    if downloaded_items.is_empty() {
        return Err(anyhow!("No files found after download/extraction"));
    }

    downloaded_items.sort_by_key(|item| item.metadata.0);

    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }

    let mut writer = WriterBuilder::new().from_path(output)?;

    let pb_style = ProgressStyle::default_bar()
        .template("{msg} [{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7}")?
        .progress_chars("##-");
    let pb = ProgressBar::new(downloaded_items.len() as u64);
    pb.set_style(pb_style);
    pb.set_message("Merging CSVs...");

    let mut expected_header: Option<Vec<String>> = None;

    for item in downloaded_items.iter() {
        let mut rdr = open_shiftjis_csv(&item.extracted_path)
            .with_context(|| format!("when opening {}", item.extracted_path.display()))?;

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

        let header = normalize_headers(&header1, &header2);
        if let Some(expected) = expected_header.as_ref() {
            if expected != &header {
                return Err(anyhow!(
                    "CSV header mismatch: {}",
                    item.extracted_path.display()
                ));
            }
        } else {
            writer
                .write_record(&header)
                .with_context(|| format!("when writing {}", output.display()))?;
            expected_header = Some(header);
        }

        for row in rdr.records() {
            let row = row?;
            writer
                .write_record(&row)
                .with_context(|| format!("when writing {}", output.display()))?;
        }

        pb.inc(1);
    }

    writer.flush()?;
    pb.finish_with_message(format!("Merged CSV written to {}", output.display()));

    Ok(())
}
