use crate::download::{self, DownloadedItem};
use anyhow::{Context, Result, anyhow};
use csv::ReaderBuilder;
use encoding_rs::SHIFT_JIS;
use encoding_rs_io::DecodeReaderBytesBuilder;
use futures::stream;
use indicatif::{ProgressBar, ProgressStyle};
use jismesh::codes::JAPAN_LV1;
use serde::Deserialize;
use std::{io::BufReader, path::Path, str::FromStr};
use tokio_postgres::{NoTls, types::ToSql};
use url::Url;

fn open_shiftjis_csv(path: &str) -> csv::Reader<Box<dyn std::io::Read>> {
    let file = std::fs::File::open(path).expect("failed to open file");
    let reader = BufReader::new(file);

    let transcoded = DecodeReaderBytesBuilder::new()
        .encoding(Some(SHIFT_JIS))
        .build(reader);

    ReaderBuilder::new()
        .has_headers(false) // we'll handle headers ourselves
        .from_reader(Box::new(transcoded))
}

fn parse_nullable<T>(value: &str) -> Result<Option<T>>
where
    T: FromStr,
    <T as FromStr>::Err: std::error::Error + Send + Sync + 'static,
{
    let v = value.trim();
    if v.is_empty() || v == "*" {
        Ok(None)
    } else {
        Ok(Some(v.parse::<T>()?))
    }
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

    /// The EPSG code the mesh code is based on.
    /// Valid values: 4301 (Tokyo Datum), 4612 (JGD2000), 6668 (JGD2011)
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

fn infer_column_type(col: &str) -> &'static str {
    if col == "KEY_CODE" || col == "HTKSAKI" {
        "BIGINT"
    } else if col == "GASSAN" {
        "BIGINT[]"
    } else if col == "HTKSYORI" {
        "SMALLINT"
    } else {
        "INTEGER"
    }
}

/// Given a path to a CSV file, create a schema in the Postgres database
/// Returns a tuple of (table name, column names)
async fn create_schema(
    client: &tokio_postgres::Client,
    mesh_stats: &MeshStats,
    file: &Path,
) -> Result<(String, Vec<String>)> {
    let mut rdr = open_shiftjis_csv(file.to_str().unwrap());

    // Read headers
    let header1 = rdr.records().next().unwrap()?; // first header row
    let header2 = rdr.records().next().unwrap()?; // second header row

    // Determine column names
    let columns: Vec<String> = header2
        .iter()
        .enumerate()
        .map(|(i, h2)| {
            let col = if h2.trim().is_empty() {
                // if header2 is empty, use header1
                // if header1 is empty, we probably have a bad CSV file.
                header1.get(i).unwrap().to_string()
            } else {
                h2.to_string()
            };

            col.trim().replace("\u{3000}", "").to_string()
        })
        .collect();

    let column_defs: Vec<String> = columns
        .iter()
        .map(|col| format!("\"{}\" {}", col, infer_column_type(col)))
        .collect();

    let table_name = format!(
        "jp_estat_mesh_{}_{}_{}",
        mesh_stats.year, mesh_stats.stats_id, mesh_stats.meshlevel,
    );
    client
        .execute(&format!("DROP TABLE IF EXISTS {}", table_name), &[])
        .await?;
    let create_stmt = format!("CREATE TABLE {} ({});", table_name, column_defs.join(", "));
    client.execute(&create_stmt, &[]).await?;

    Ok((table_name, columns))
}

async fn import_csv_to_postgres(
    client: &mut tokio_postgres::Client,
    file: &Path,
    table_name: &str,
    columns: &[String],
) -> Result<()> {
    let mut rdr = open_shiftjis_csv(file.to_str().unwrap());
    let insert_sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        table_name,
        columns
            .iter()
            .map(|c| format!("\"{}\"", c))
            .collect::<Vec<_>>()
            .join(", "),
        columns
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let insert_stmt = client.prepare(&insert_sql).await?;

    let tx = client.transaction().await?;

    // Skip the first two header rows
    rdr.records().next().unwrap()?;
    rdr.records().next().unwrap()?;

    for result in rdr.records() {
        let record = result?;
        let mut params: Vec<Box<dyn ToSql + Sync>> = Vec::with_capacity(columns.len());
        for (i, col) in columns.iter().enumerate() {
            let value = record.get(i).unwrap_or("");
            if col == "KEY_CODE" || col == "HTKSAKI" {
                params.push(Box::new(parse_nullable::<i64>(value)?));
            } else if col == "HTKSYORI" {
                params.push(Box::new(parse_nullable::<i16>(value)?));
            } else if col == "GASSAN" {
                if value.is_empty() {
                    params.push(Box::new(None::<Vec<i64>>));
                } else {
                    let values: Vec<i64> = value
                        .split(';')
                        .map(|s| s.parse::<_>())
                        .collect::<Result<Vec<_>, _>>()?;
                    params.push(Box::new(values));
                }
            } else {
                params.push(Box::new(parse_nullable::<i32>(value)?));
            }
        }
        tx.execute(
            &insert_stmt,
            &params.iter().map(|p| p.as_ref()).collect::<Vec<_>>(),
        )
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub async fn process_mesh(
    postgres_url: &str,
    tmp_dir: &Path,
    level: u8,
    year: u16,
    survey: &str,
) -> Result<()> {
    let mesh_stats = get_matching_mesh_stats(level, year, survey)
        .ok_or(anyhow!("一致する統計データが見つかりません"))?;

    // Prepare items for download
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

    // Use the generic download function
    let downloaded_items: Vec<DownloadedItem<(u64, Url)>> = download::download_and_extract_all(
        stream::iter(urls_with_metadata),
        |(_mesh, url)| url.clone(),
        |(mesh, _url)| format!("{}-{}-{}.zip", mesh_stats.year, mesh_stats.stats_id, mesh),
        download::DownloadOptions {
            target_ext: "txt",
            tmp_dir,
            download_message: "Downloading Mesh CSVs...",
            extract_message: "Extracting Mesh CSVs...",
            concurrency: 10,
        },
    )
    .await?;

    println!("Files downloaded and extracted.");

    let first_extracted_path = downloaded_items
        .first()
        .map(|item| item.extracted_path.clone())
        .ok_or(anyhow!("No files found after download/extraction"))?;

    let (mut client, connection) = tokio_postgres::connect(postgres_url, NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("DB error: {}", e);
        }
    });

    let (table_name, columns) = create_schema(&client, mesh_stats, &first_extracted_path).await?;
    println!("Schema created: {}", table_name);

    let pb_style = ProgressStyle::default_bar()
        .template("{msg} [{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7}")?
        .progress_chars("##-");
    let pb = ProgressBar::new(downloaded_items.len() as u64);
    pb.set_style(pb_style);
    pb.set_message("Importing CSVs...");
    for item in downloaded_items.iter() {
        import_csv_to_postgres(&mut client, &item.extracted_path, &table_name, &columns)
            .await
            .with_context(|| format!("when importing {}", item.extracted_path.display()))?;
        pb.inc(1);
    }
    pb.finish();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::AVAILABLE;

    #[test]
    fn available_mesh_stats_are_loaded() {
        assert!(!AVAILABLE.is_empty());
    }
}
