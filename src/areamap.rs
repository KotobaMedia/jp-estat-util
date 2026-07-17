use anyhow::{Context as _, Result, bail};
use futures::stream;
use indicatif::{ProgressBar, ProgressStyle};
use km_to_sql::metadata::{ColumnMetadata, TableMetadata};
use std::path::Path;
use tokio_postgres::NoTls;
use url::Url;

use crate::{
    download::{self, DownloadedItem},
    gdal,
};

const PREF_CODES: [&str; 47] = [
    "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13", "14", "15", "16",
    "17", "18", "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31", "32",
    "33", "34", "35", "36", "37", "38", "39", "40", "41", "42", "43", "44", "45", "46", "47",
];

#[derive(Clone, Debug)]
pub struct DlServey<'a> {
    year: u32,
    id: &'a str,
    datum: &'a str,
}

const DL_SERVEY_IDS: [DlServey; 5] = [
    DlServey {
        year: 2020,
        id: "A002005212020",
        datum: "2011",
    }, // 2020年
    DlServey {
        year: 2015,
        id: "A002005212015",
        datum: "2011",
    }, // 2015年
    DlServey {
        year: 2010,
        id: "A002005212010",
        datum: "2000",
    }, // 2010年
    DlServey {
        year: 2005,
        id: "A002005212005",
        datum: "2000",
    }, // 2005年
    DlServey {
        year: 2000,
        id: "A002005212000",
        datum: "2000",
    }, // 2000年
];

const AREAMAP_OGR2OGR_WHERE: &str = "HCODE IS NULL OR HCODE <> 8154";

fn get_shape_url(dlservey_id: &str, code: &str, datum: &str) -> String {
    format!(
        "https://www.e-stat.go.jp/gis/statmap-search/data?dlserveyId={}&code={}&coordSys=1&format=shape&downloadType=5&datum={}",
        dlservey_id, code, datum
    )
}

#[derive(Clone, Debug)]
struct ShapeUrlMeta {
    dlservey: DlServey<'static>,
    pref_code: &'static str,
    url: Url,
}

fn get_target_serveys(survey_year: Option<u32>) -> Result<Vec<DlServey<'static>>> {
    if let Some(year) = survey_year {
        if let Some(servey) = DL_SERVEY_IDS.iter().find(|servey| servey.year == year) {
            return Ok(vec![servey.clone()]);
        }
        let available_years = DL_SERVEY_IDS
            .iter()
            .map(|servey| servey.year.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "Unsupported survey year: {}. Available years: {}",
            year,
            available_years
        );
    }
    Ok(DL_SERVEY_IDS.to_vec())
}

fn get_all_shape_urls(target_serveys: &[DlServey<'static>]) -> Vec<ShapeUrlMeta> {
    let mut urls = Vec::new();
    for code in PREF_CODES.iter() {
        for dlservey in target_serveys.iter() {
            let url_str = get_shape_url(dlservey.id, code, dlservey.datum);
            urls.push(ShapeUrlMeta {
                dlservey: dlservey.clone(),
                pref_code: code,
                url: Url::parse(&url_str).expect("Failed to parse shape URL"),
            });
        }
    }
    urls
}

fn is_single_layer_output(output: &str, output_format: Option<&str>) -> bool {
    if as_postgres_url(output, output_format).is_some() {
        return false;
    }

    if output_format
        .map(|v| {
            v.eq_ignore_ascii_case("parquet")
                || v.eq_ignore_ascii_case("geojson")
                || v.eq_ignore_ascii_case("flatgeobuf")
                || v.eq_ignore_ascii_case("csv")
        })
        .unwrap_or(false)
    {
        return true;
    }

    Path::new(output)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            ext.eq_ignore_ascii_case("parquet")
                || ext.eq_ignore_ascii_case("geojson")
                || ext.eq_ignore_ascii_case("json")
                || ext.eq_ignore_ascii_case("fgb")
                || ext.eq_ignore_ascii_case("csv")
        })
        .unwrap_or(false)
}

fn output_layer_name_from_destination(output: &str) -> Option<String> {
    Path::new(output)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(|stem| stem.to_string())
}

async fn import_shapes(
    downloaded_shapes: Vec<DownloadedItem<ShapeUrlMeta>>,
    target_serveys: &[DlServey<'static>],
    output: &str,
    output_format: Option<&str>,
    output_layer_name: Option<&str>,
    output_crs: Option<&str>,
    tmp_dir: &Path,
) -> Result<()> {
    let pb = ProgressBar::new(target_serveys.len() as u64);
    let bar_style = ProgressStyle::default_bar()
        .template("{msg} [{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7}")?
        .progress_chars("##-");
    pb.set_style(bar_style);
    pb.set_message("Importing shapes with ogr2ogr...");

    for servey in target_serveys.iter() {
        let shapes_for_year = downloaded_shapes
            .iter()
            .filter(|item| item.metadata.dlservey.year == servey.year)
            .map(|item| item.extracted_path.clone())
            .collect::<Vec<_>>();

        if shapes_for_year.is_empty() {
            println!(
                "No shapes found for year {}, skipping VRT creation and import.",
                servey.year
            );
            pb.inc(1);
            continue;
        }

        let vrt_path = tmp_dir.join(format!("jp_estat_areamap_{}.vrt", servey.year));
        gdal::create_vrt(&vrt_path, &shapes_for_year)
            .await
            .with_context(|| format!("when creating VRT: {}", vrt_path.display()))?;
        gdal::load(
            &vrt_path,
            output,
            output_format,
            output_layer_name,
            Some(AREAMAP_OGR2OGR_WHERE),
            output_crs,
        )
        .await
        .with_context(|| format!("when loading VRT: {}", vrt_path.display()))?;
        pb.inc(1);
    }

    println!("All imports completed.");
    Ok(())
}

fn as_postgres_url<'a>(output: &'a str, output_format: Option<&str>) -> Option<&'a str> {
    if let Some(stripped) = output
        .strip_prefix("PG:")
        .or_else(|| output.strip_prefix("pg:"))
    {
        return Some(stripped);
    }
    if output_format
        .map(|v| v.eq_ignore_ascii_case("postgresql"))
        .unwrap_or(false)
    {
        return Some(output);
    }
    None
}

async fn insert_postgres_metadata(
    postgres_url: &str,
    target_serveys: &[DlServey<'static>],
    output_crs: Option<&str>,
) -> Result<()> {
    let (client, connection) = tokio_postgres::connect(postgres_url, NoTls)
        .await
        .with_context(|| "when connecting to PostgreSQL")?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            panic!("PostgreSQL connection error: {}", e);
        }
    });

    km_to_sql::postgres::init_schema(&client).await?;

    if let Some(crs) = output_crs
        && parse_output_srid(crs).is_none() {
            println!(
                "Warning: could not infer EPSG SRID from --output-crs='{}'. PostgreSQL metadata will use geometry(polygon) without SRID.",
                crs
            );
        }

    for servey in target_serveys.iter() {
        let table_name = format!("jp_estat_areamap_{}", servey.year);
        let geom_data_type = metadata_geom_data_type(servey, output_crs);

        let columns: Vec<ColumnMetadata> = vec![
            ColumnMetadata {
                name: "ogc_fid".to_string(),
                desc: None,
                data_type: "integer".to_string(),
                foreign_key: None,
                enum_values: None,
            },
            ColumnMetadata {
                name: "geom".to_string(),
                desc: Some("Geometry".to_string()),
                data_type: geom_data_type,
                foreign_key: None,
                enum_values: None,
            },
            ColumnMetadata {
                name: "key_code".to_string(),
                desc: Some("小地域コード".to_string()),
                data_type: "varchar(255)".to_string(),
                foreign_key: None,
                enum_values: None,
            },
            ColumnMetadata {
                name: "pref_name".to_string(),
                desc: Some("都道府県名".to_string()),
                data_type: "varchar(255)".to_string(),
                foreign_key: None,
                enum_values: None,
            },
            ColumnMetadata {
                name: "city_name".to_string(),
                desc: Some("市区町村名".to_string()),
                data_type: "varchar(255)".to_string(),
                foreign_key: None,
                enum_values: None,
            },
            ColumnMetadata {
                name: "s_name".to_string(),
                desc: Some("小地域名".to_string()),
                data_type: "varchar(255)".to_string(),
                foreign_key: None,
                enum_values: None,
            },
            ColumnMetadata {
                name: "jinko".to_string(),
                desc: Some("人口".to_string()),
                data_type: "int".to_string(),
                foreign_key: None,
                enum_values: None,
            },
            ColumnMetadata {
                name: "setai".to_string(),
                desc: Some("世帯数".to_string()),
                data_type: "int".to_string(),
                foreign_key: None,
                enum_values: None,
            },
        ];

        let metadata = TableMetadata {
            name: format!("国勢調査 {}年 小地域境界データ", servey.year),
            desc: Some(
                "丁目・大字・小字などの境界ポリゴンと、簡易的な人口データが含まれている"
                    .to_string(),
            ),
            source: Some("総務省統計局".to_string()),
            source_url: Some(Url::parse(
                "https://www.e-stat.go.jp/gis/statmap-search?page=1&type=2&aggregateUnitForBoundary=A&toukeiCode=00200521",
            ).unwrap()),
            license: None,
            license_url: Some(Url::parse("https://www.e-stat.go.jp/terms-of-use").unwrap()),
            primary_key: Some("ogc_fid".to_string()),
            columns,
        };
        km_to_sql::postgres::upsert(&client, &table_name, &metadata).await?;
    }

    Ok(())
}

fn default_geom_srid(datum: &str) -> i32 {
    if datum == "2000" {
        4621 // 日本測地系2000
    } else {
        6668 // 日本測地系2011
    }
}

fn parse_output_srid(output_crs: &str) -> Option<i32> {
    let value = output_crs.trim();
    if value.is_empty() {
        return None;
    }

    if let Ok(srid) = value.parse::<i32>() {
        return Some(srid);
    }

    let upper_value = value.to_ascii_uppercase();
    for marker in ["EPSG::", "EPSG:"] {
        if let Some((_, rest)) = upper_value.rsplit_once(marker) {
            let digits = rest
                .trim_start_matches(':')
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            if !digits.is_empty()
                && let Ok(srid) = digits.parse::<i32>() {
                    return Some(srid);
                }
        }
    }

    None
}

fn metadata_geom_data_type(servey: &DlServey<'_>, output_crs: Option<&str>) -> String {
    match output_crs {
        Some(crs) => match parse_output_srid(crs) {
            Some(srid) => format!("geometry(polygon, {})", srid),
            None => "geometry(polygon)".to_string(),
        },
        None => format!("geometry(polygon, {})", default_geom_srid(servey.datum)),
    }
}

pub async fn process_areamap(
    output: &str,
    output_format: Option<&str>,
    output_crs: Option<&str>,
    tmp_dir: &Path,
    survey_year: Option<u32>,
) -> Result<()> {
    let target_serveys = get_target_serveys(survey_year)?;
    let single_layer_output = is_single_layer_output(output, output_format);
    if single_layer_output && target_serveys.len() > 1 {
        bail!(
            "Output '{}' appears to be a single-layer format. Use `--year` to export a single survey year.",
            output
        );
    }

    let output_layer_name = if single_layer_output && target_serveys.len() == 1 {
        output_layer_name_from_destination(output)
    } else {
        None
    };

    gdal::ensure_available()
        .await
        .with_context(|| "when checking GDAL availability with `ogrinfo --version`")?;

    // 1. Get URLs and metadata
    let shape_url_metas = get_all_shape_urls(&target_serveys);

    // 2. Download all shapes and unzip them using the generic function
    let downloaded_items: Vec<DownloadedItem<ShapeUrlMeta>> = download::download_and_extract_all(
        stream::iter(shape_url_metas),
        |meta| meta.url.clone(),
        |meta| format!("{}-{}.zip", meta.dlservey.year, meta.pref_code),
        "shp", // Target extension is .shp
        tmp_dir,
        "Downloading Shapes...",
        "Extracting Shapes...",
        10, // Concurrency level
    )
    .await
    .with_context(|| "when downloading and extracting shapes".to_string())?;

    // 3. Import the shapefiles using ogr2ogr
    import_shapes(
        downloaded_items,
        &target_serveys,
        output,
        output_format,
        output_layer_name.as_deref(),
        output_crs,
        tmp_dir,
    )
    .await
    .with_context(|| "when importing to ogr2ogr".to_string())?;

    // 4. For PostgreSQL outputs, insert metadata
    if let Some(postgres_url) = as_postgres_url(output, output_format) {
        insert_postgres_metadata(postgres_url, &target_serveys, output_crs).await?;
    } else {
        println!(
            "PostgreSQL metadata insertion was skipped because output is not a PostgreSQL datasource."
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_single_layer_output, output_layer_name_from_destination, parse_output_srid};

    #[test]
    fn detects_single_layer_by_extension() {
        assert!(is_single_layer_output("./output/areamap.parquet", None));
        assert!(is_single_layer_output("./output/areamap.geojson", None));
        assert!(!is_single_layer_output("./output/areamap.gpkg", None));
    }

    #[test]
    fn detects_single_layer_by_output_format() {
        assert!(is_single_layer_output(
            "./output/areamap.gpkg",
            Some("Parquet")
        ));
        assert!(!is_single_layer_output(
            "PG:host=127.0.0.1 dbname=jp_estat",
            Some("Parquet")
        ));
    }

    #[test]
    fn derives_layer_name_from_output_path() {
        assert_eq!(
            output_layer_name_from_destination("./output/areamap.parquet"),
            Some("areamap".to_string())
        );
        assert_eq!(output_layer_name_from_destination(""), None);
    }

    #[test]
    fn parses_output_srid_from_common_formats() {
        assert_eq!(parse_output_srid("4326"), Some(4326));
        assert_eq!(parse_output_srid("EPSG:6668"), Some(6668));
        assert_eq!(parse_output_srid("urn:ogc:def:crs:EPSG::4612"), Some(4612));
        assert_eq!(parse_output_srid("epsg:3857"), Some(3857));
        assert_eq!(parse_output_srid("CRS84"), None);
    }
}
