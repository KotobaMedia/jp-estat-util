use crate::download::{self, DownloadedItem};
use anyhow::{Context, Result, anyhow, bail};
use csv::{ReaderBuilder, StringRecord};
use encoding_rs::SHIFT_JIS;
use encoding_rs_io::DecodeReaderBytesBuilder;
use futures::stream;
use indicatif::{ProgressBar, ProgressStyle};
use jismesh::{MeshLevel, codes::JAPAN_LV1, to_meshlevel};
use mesh_data_tile::{
    CompressionMode, DType, Endianness, MeshKind, TileDimensions, TileEncodeInput, encode_tile,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::BufReader,
    path::Path,
};
use url::Url;

const DATA_COLUMN_START: usize = 4;
const NO_DATA_I32: i32 = i32::MIN;

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
#[cfg_attr(
    test,
    expect(dead_code, reason = "the test harness does not invoke this command")
)]
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

#[derive(Debug, Serialize)]
struct TileSetMetadata {
    format: &'static str,
    tile_file_pattern: &'static str,
    mesh_kind: &'static str,
    data_mesh_level: u8,
    tile_mesh_level: u8,
    data_mesh_level_name: String,
    tile_mesh_level_name: String,
    year: u16,
    survey: String,
    stats_id: String,
    rows: u32,
    cols: u32,
    bands: u8,
    dtype: &'static str,
    endianness: &'static str,
    compression: &'static str,
    no_data: i32,
    band_columns: Vec<BandColumnMetadata>,
}

#[derive(Debug, Serialize)]
struct BandColumnMetadata {
    band: u16,
    name: String,
}

#[derive(Debug, Clone)]
struct SelectedBand {
    source_idx: usize,
    name: String,
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

fn build_available_bands(
    header_codes: &[String],
    normalized_header: &[String],
) -> Result<Vec<SelectedBand>> {
    if header_codes.len() != normalized_header.len() {
        bail!("header column count mismatch");
    }
    if header_codes.len() <= DATA_COLUMN_START {
        bail!("no stat columns found");
    }

    let mut bands = Vec::with_capacity(header_codes.len() - DATA_COLUMN_START);
    for (source_idx, name) in normalized_header.iter().enumerate().skip(DATA_COLUMN_START) {
        bands.push(SelectedBand {
            source_idx,
            name: name.clone(),
        });
    }
    Ok(bands)
}

fn resolve_selected_bands(
    available_bands: &[SelectedBand],
    requested_bands: Option<&[String]>,
) -> Result<Vec<SelectedBand>> {
    if available_bands.is_empty() {
        bail!("no selectable bands available");
    }

    let requested_bands = match requested_bands {
        Some(v) => v,
        None => return Ok(available_bands.to_vec()),
    };
    if requested_bands.is_empty() {
        bail!("--bands was provided but no bands were specified");
    }

    let mut selected = Vec::with_capacity(requested_bands.len());
    let mut used_source_indices = HashSet::new();
    for requested in requested_bands {
        let key = requested.trim();
        if key.is_empty() {
            bail!("--bands contains an empty value");
        }

        let band = available_bands
            .iter()
            .find(|b| b.name == key)
            .ok_or_else(|| anyhow!("unknown band '{}'", key))?;
        if !used_source_indices.insert(band.source_idx) {
            bail!("duplicate band in --bands: {}", key);
        }

        selected.push(band.clone());
    }

    Ok(selected)
}

fn digits_for_level(level: u8) -> Result<usize> {
    match level {
        1 => Ok(4),
        2 => Ok(6),
        3 => Ok(8),
        4 => Ok(9),
        5 => Ok(10),
        6 => Ok(11),
        _ => bail!("unsupported mesh level: {}", level),
    }
}

fn refinement_factor(next_level: u8) -> Result<usize> {
    match next_level {
        2 => Ok(8),
        3 => Ok(10),
        4..=6 => Ok(2),
        _ => bail!("unsupported refinement step to level {}", next_level),
    }
}

fn subdivisions_per_axis(tile_level: u8, data_level: u8) -> Result<usize> {
    if tile_level > data_level {
        bail!(
            "tile-level ({}) must be <= data level ({})",
            tile_level,
            data_level
        );
    }

    let mut size = 1usize;
    for next_level in (tile_level + 1)..=data_level {
        size = size
            .checked_mul(refinement_factor(next_level)?)
            .ok_or(anyhow!("tile resolution overflow"))?;
    }

    Ok(size)
}

fn parse_digit(bytes: &[u8], idx: usize) -> Result<u8> {
    let b = bytes
        .get(idx)
        .ok_or(anyhow!("mesh code is shorter than expected"))?;
    if !b.is_ascii_digit() {
        bail!("mesh code contains non-digit character at position {}", idx);
    }
    Ok(*b - b'0')
}

fn decode_quadrant(q: u8) -> Result<(usize, usize)> {
    match q {
        1 => Ok((0, 0)), // southwest
        2 => Ok((0, 1)), // southeast
        3 => Ok((1, 0)), // northwest
        4 => Ok((1, 1)), // northeast
        _ => bail!("invalid split mesh quadrant: {}", q),
    }
}

fn mesh_level_to_u8(level: MeshLevel) -> Option<u8> {
    match level {
        MeshLevel::Lv1 => Some(1),
        MeshLevel::Lv2 => Some(2),
        MeshLevel::Lv3 => Some(3),
        MeshLevel::Lv4 => Some(4),
        MeshLevel::Lv5 => Some(5),
        MeshLevel::Lv6 => Some(6),
        _ => None,
    }
}

fn mesh_level_from_u8(level: u8) -> Result<MeshLevel> {
    match level {
        1 => Ok(MeshLevel::Lv1),
        2 => Ok(MeshLevel::Lv2),
        3 => Ok(MeshLevel::Lv3),
        4 => Ok(MeshLevel::Lv4),
        5 => Ok(MeshLevel::Lv5),
        6 => Ok(MeshLevel::Lv6),
        _ => bail!("unsupported standard mesh level: {}", level),
    }
}

fn validate_mesh_code_level(mesh_code: u64, expected_level: u8) -> Result<()> {
    let levels = to_meshlevel(&[mesh_code])
        .map_err(|e| anyhow!("failed to parse mesh code {}: {}", mesh_code, e))?;
    let actual_level = levels
        .first()
        .copied()
        .ok_or(anyhow!("mesh level parse result was empty"))?;
    let actual_level_u8 = mesh_level_to_u8(actual_level).ok_or(anyhow!(
        "mesh code {} is not a supported standard level (actual: {:?})",
        mesh_code,
        actual_level
    ))?;

    if actual_level_u8 != expected_level {
        bail!(
            "mesh code {} has level {}, expected {}",
            mesh_code,
            actual_level_u8,
            expected_level
        );
    }

    Ok(())
}

fn map_meshcode_to_tile(
    mesh_code: u64,
    data_level: u8,
    tile_level: u8,
    rows_per_axis: usize,
) -> Result<(u64, usize, usize)> {
    let code_str = mesh_code.to_string();
    let expected_digits = digits_for_level(data_level)?;
    if code_str.len() != expected_digits {
        bail!(
            "mesh code {} has {} digits, expected {} for level {}",
            mesh_code,
            code_str.len(),
            expected_digits,
            data_level
        );
    }

    let tile_digits = digits_for_level(tile_level)?;
    let tile_code: u64 = code_str[..tile_digits]
        .parse()
        .with_context(|| format!("failed to parse parent tile code from {}", mesh_code))?;

    let bytes = code_str.as_bytes();
    let mut row_south = 0usize;
    let mut col = 0usize;

    for next_level in (tile_level + 1)..=data_level {
        let factor = refinement_factor(next_level)?;
        let (sub_row, sub_col) = match next_level {
            2 => {
                let r = parse_digit(bytes, 4)?;
                let c = parse_digit(bytes, 5)?;
                if r > 7 || c > 7 {
                    bail!("invalid Lv2 subdivision in mesh code {}", mesh_code);
                }
                (usize::from(r), usize::from(c))
            }
            3 => {
                let r = parse_digit(bytes, 6)?;
                let c = parse_digit(bytes, 7)?;
                if r > 9 || c > 9 {
                    bail!("invalid Lv3 subdivision in mesh code {}", mesh_code);
                }
                (usize::from(r), usize::from(c))
            }
            4 => decode_quadrant(parse_digit(bytes, 8)?)?,
            5 => decode_quadrant(parse_digit(bytes, 9)?)?,
            6 => decode_quadrant(parse_digit(bytes, 10)?)?,
            _ => bail!("unsupported mesh level {}", next_level),
        };

        row_south = row_south * factor + sub_row;
        col = col * factor + sub_col;
    }

    if row_south >= rows_per_axis || col >= rows_per_axis {
        bail!(
            "computed tile coordinates out of range for mesh code {} (row_south={}, col={}, rows={})",
            mesh_code,
            row_south,
            col,
            rows_per_axis
        );
    }

    let row_top = rows_per_axis - 1 - row_south;
    Ok((tile_code, row_top, col))
}

fn parse_stat_value(value: &str) -> Result<i32> {
    let v = value.trim();
    if v.is_empty() || v == "*" {
        return Ok(NO_DATA_I32);
    }

    let parsed = v
        .parse::<i64>()
        .with_context(|| format!("invalid integer value: {}", v))?;
    if parsed < i64::from(i32::MIN) || parsed > i64::from(i32::MAX) {
        bail!("value out of i32 range: {}", parsed);
    }

    Ok(parsed as i32)
}

fn build_payload_i32(values: &[i32]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    payload
}

async fn write_tile(
    output_dir: &Path,
    tile_code: u64,
    rows_per_axis: usize,
    band_count: usize,
    values: &[i32],
) -> Result<()> {
    let payload = build_payload_i32(values);

    let rows = u32::try_from(rows_per_axis).context("tile rows exceed u32")?;
    let cols = u32::try_from(rows_per_axis).context("tile cols exceed u32")?;
    let bands = u8::try_from(band_count).context("band count exceeds u8")?;

    let encoded = encode_tile(TileEncodeInput {
        tile_id: tile_code,
        mesh_kind: MeshKind::JisX0410,
        dtype: DType::Int32,
        endianness: Endianness::Little,
        compression: CompressionMode::DeflateRaw,
        dimensions: TileDimensions { rows, cols, bands },
        no_data: Some(NO_DATA_I32 as f64),
        payload: &payload,
    })
    .map_err(|e| anyhow!("failed to encode tile {}: {}", tile_code, e))?;

    let output_path = output_dir.join(format!("{}.tile", tile_code));
    tokio::fs::write(&output_path, encoded.bytes)
        .await
        .with_context(|| format!("failed to write {}", output_path.display()))?;

    Ok(())
}

async fn write_metadata(
    output_dir: &Path,
    mesh_stats: &MeshStats,
    survey: &str,
    data_level: u8,
    tile_level: u8,
    rows_per_axis: usize,
    band_names: &[String],
) -> Result<()> {
    let rows = u32::try_from(rows_per_axis).context("tile rows exceed u32")?;
    let cols = u32::try_from(rows_per_axis).context("tile cols exceed u32")?;
    let bands = u8::try_from(band_names.len()).context("band count exceeds u8")?;

    let data_mesh_level = mesh_level_from_u8(data_level)?;
    let tile_mesh_level = mesh_level_from_u8(tile_level)?;
    let band_columns: Vec<BandColumnMetadata> = band_names
        .iter()
        .enumerate()
        .map(|(i, name)| BandColumnMetadata {
            band: (i + 1) as u16,
            name: name.clone(),
        })
        .collect();

    let metadata = TileSetMetadata {
        format: "MTI1",
        tile_file_pattern: "{meshcode}.tile",
        mesh_kind: "jis-x0410",
        data_mesh_level: data_level,
        tile_mesh_level: tile_level,
        data_mesh_level_name: data_mesh_level.to_string(),
        tile_mesh_level_name: tile_mesh_level.to_string(),
        year: mesh_stats.year,
        survey: survey.to_string(),
        stats_id: mesh_stats.stats_id.clone(),
        rows,
        cols,
        bands,
        dtype: "int32",
        endianness: "little",
        compression: "deflate-raw",
        no_data: NO_DATA_I32,
        band_columns,
    };

    let metadata_path = output_dir.join("metadata.json");
    let body = serde_json::to_vec_pretty(&metadata)?;
    tokio::fs::write(&metadata_path, body)
        .await
        .with_context(|| format!("failed to write {}", metadata_path.display()))?;

    Ok(())
}

pub async fn process_mesh_tile(
    tmp_dir: &Path,
    level: u8,
    year: u16,
    survey: &str,
    tile_level: Option<u8>,
    bands: Option<&[String]>,
    output_dir: &Path,
) -> Result<()> {
    let tile_level = tile_level.unwrap_or(level);
    if tile_level > level {
        bail!(
            "tile-level ({}) must be <= data level ({})",
            tile_level,
            level
        );
    }

    // Validate supported level inputs through jismesh-level conversion.
    let _ = mesh_level_from_u8(level)?;
    let _ = mesh_level_from_u8(tile_level)?;

    let rows_per_axis = subdivisions_per_axis(tile_level, level)?;
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
        download::DownloadOptions {
            target_ext: "txt",
            tmp_dir,
            download_message: "Downloading Mesh CSVs...",
            extract_message: "Extracting Mesh CSVs...",
            concurrency: 10,
        },
    )
    .await?;

    if downloaded_items.is_empty() {
        return Err(anyhow!("No files found after download/extraction"));
    }

    tokio::fs::create_dir_all(output_dir).await?;
    downloaded_items.sort_by_key(|item| item.metadata.0);

    let pb_style = ProgressStyle::default_bar()
        .template("{msg} [{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7}")?
        .progress_chars("##-");
    let pb = ProgressBar::new(downloaded_items.len() as u64);
    pb.set_style(pb_style);
    pb.set_message("Encoding mesh tiles...");

    let mut expected_header: Option<Vec<String>> = None;
    let mut selected_bands: Vec<SelectedBand> = Vec::new();
    let mut total_tiles = 0usize;

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

        let normalized_header = normalize_headers(&header1, &header2);
        if normalized_header.len() <= DATA_COLUMN_START {
            bail!("CSV has too few columns: {}", item.extracted_path.display());
        }

        if let Some(expected) = expected_header.as_ref() {
            if expected != &normalized_header {
                bail!("CSV header mismatch: {}", item.extracted_path.display());
            }
        } else {
            let header_codes: Vec<String> = header1.iter().map(|s| s.trim().to_string()).collect();
            let available_bands = build_available_bands(&header_codes, &normalized_header)
                .with_context(|| {
                    format!(
                        "when reading headers from {}",
                        item.extracted_path.display()
                    )
                })?;
            selected_bands = resolve_selected_bands(&available_bands, bands)?;
            if selected_bands.len() > usize::from(u8::MAX) {
                bail!(
                    "too many columns for tile bands ({} > {})",
                    selected_bands.len(),
                    u8::MAX
                );
            }

            let metadata_band_names: Vec<String> =
                selected_bands.iter().map(|b| b.name.clone()).collect();

            write_metadata(
                output_dir,
                mesh_stats,
                survey,
                level,
                tile_level,
                rows_per_axis,
                &metadata_band_names,
            )
            .await?;

            expected_header = Some(normalized_header);
        }

        let band_count = selected_bands.len();
        let pixels = rows_per_axis
            .checked_mul(rows_per_axis)
            .ok_or(anyhow!("tile pixel count overflow"))?;
        let tile_value_count = pixels
            .checked_mul(band_count)
            .ok_or(anyhow!("tile payload size overflow"))?;

        let mut tiles: BTreeMap<u64, Vec<i32>> = BTreeMap::new();
        let mut validated_this_file = false;

        for row in rdr.records() {
            let row = row?;
            let code_str = row.get(0).unwrap_or("").trim();
            if code_str.is_empty() {
                continue;
            }

            let mesh_code: u64 = code_str.parse().with_context(|| {
                format!(
                    "invalid mesh code '{}' in {}",
                    code_str,
                    item.extracted_path.display()
                )
            })?;

            // Validate at least one row per file using jismesh parsing.
            if !validated_this_file {
                validate_mesh_code_level(mesh_code, level).with_context(|| {
                    format!(
                        "mesh code level mismatch in {}",
                        item.extracted_path.display()
                    )
                })?;
                validated_this_file = true;
            }

            let (tile_code, row_idx, col_idx) =
                map_meshcode_to_tile(mesh_code, level, tile_level, rows_per_axis).with_context(
                    || {
                        format!(
                            "failed to map mesh code {} from {}",
                            mesh_code,
                            item.extracted_path.display()
                        )
                    },
                )?;

            let tile = tiles
                .entry(tile_code)
                .or_insert_with(|| vec![NO_DATA_I32; tile_value_count]);
            let base_idx = ((row_idx * rows_per_axis) + col_idx) * band_count;

            for (band_idx, band) in selected_bands.iter().enumerate() {
                let raw = row.get(band.source_idx).unwrap_or("");
                let value = parse_stat_value(raw).with_context(|| {
                    format!(
                        "invalid value in column '{}' for mesh code {}",
                        band.name, mesh_code
                    )
                })?;
                tile[base_idx + band_idx] = value;
            }
        }

        for (tile_code, values) in tiles.into_iter() {
            write_tile(output_dir, tile_code, rows_per_axis, band_count, &values).await?;
            total_tiles += 1;
        }

        pb.inc(1);
    }

    pb.finish_with_message(format!(
        "Mesh tile encoding completed ({} tiles)",
        total_tiles
    ));

    println!("Tile directory: {}", output_dir.display());
    println!(
        "Tile mesh level: Lv{} (data level: Lv{}, rows/cols: {})",
        tile_level, level, rows_per_axis
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subdivisions_per_axis() {
        assert_eq!(subdivisions_per_axis(1, 3).unwrap(), 80);
        assert_eq!(subdivisions_per_axis(1, 4).unwrap(), 160);
        assert_eq!(subdivisions_per_axis(3, 6).unwrap(), 8);
        assert_eq!(subdivisions_per_axis(6, 6).unwrap(), 1);
    }

    #[test]
    fn test_map_lv3_to_lv1() {
        let (tile_code, row, col) = map_meshcode_to_tile(53393599, 3, 1, 80).unwrap();
        assert_eq!(tile_code, 5339);
        assert_eq!(row, 40);
        assert_eq!(col, 59);
    }

    #[test]
    fn test_map_lv6_to_lv3() {
        let (tile_code, row, col) = map_meshcode_to_tile(53370000242, 6, 3, 8).unwrap();
        assert_eq!(tile_code, 53370000);
        assert_eq!(row, 5);
        assert_eq!(col, 7);
    }

    fn sample_available_bands() -> Vec<SelectedBand> {
        vec![
            SelectedBand {
                source_idx: 4,
                name: "人口（総数）".to_string(),
            },
            SelectedBand {
                source_idx: 5,
                name: "人口（総数）男".to_string(),
            },
            SelectedBand {
                source_idx: 6,
                name: "人口（総数）女".to_string(),
            },
        ]
    }

    #[test]
    fn test_resolve_selected_bands_default_all() {
        let available = sample_available_bands();
        let selected = resolve_selected_bands(&available, None).unwrap();
        let names: Vec<String> = selected.into_iter().map(|b| b.name).collect();
        assert_eq!(
            names,
            vec!["人口（総数）", "人口（総数）男", "人口（総数）女"]
        );
    }

    #[test]
    fn test_resolve_selected_bands_custom_order() {
        let available = sample_available_bands();
        let requested = vec!["人口（総数）女".to_string(), "人口（総数）".to_string()];
        let selected = resolve_selected_bands(&available, Some(&requested)).unwrap();
        let names: Vec<String> = selected.into_iter().map(|b| b.name).collect();
        assert_eq!(names, vec!["人口（総数）女", "人口（総数）"]);
    }

    #[test]
    fn test_resolve_selected_bands_unknown() {
        let available = sample_available_bands();
        let requested = vec!["UNKNOWN".to_string()];
        let err = resolve_selected_bands(&available, Some(&requested)).unwrap_err();
        assert!(err.to_string().contains("unknown band"));
    }
}
