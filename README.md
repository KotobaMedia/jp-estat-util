# jp-estat-util

e-Stat でホスティングしている統計情報や地理情報をSQLデータベースに取り込むツール

GISデータについては、[e-Statのデータ注意情報](https://www.e-stat.go.jp/help/data-definition-information/download) を留意ください。

## 概要

このツールは、e-Stat（政府統計の総合窓口）から統計データと地理情報をダウンロードし、サブコマンドに応じて PostgreSQL への取り込みやファイル出力を行うコマンドラインツールです。

## インストール方法

このツールは Rust で書かれていますが、コンパイル済みバイナリも下記のアーキテクチャで用意しています。

- macOS (Apple Silicon)
- Windows (x86_64)
- Linux (x86_64)

[最新の Release](https://github.com/KotobaMedia/jp-estat-util/releases/latest) から利用環境向けの zip アーカイブをダウンロードして解凍すると、コマンドラインから `jp-estat-util` を実行できます。

Rust 環境をお持ちの方は、GitHub から直接インストールできます。

```shell
cargo install --git https://github.com/KotobaMedia/jp-estat-util.git jp-estat-util
```

このリポジトリを clone 済みであれば、ローカルソースからインストールすることもできます。

```shell
cargo install --path .
```

> [!NOTE]
> `areamap` サブコマンドでは `ogr2ogr` が必要です。`mesh` など PostgreSQL に取り込むサブコマンドでは、接続先の PostgreSQL を別途用意してください。

## 使用方法

### 基本構文

```shell
jp-estat-util [OPTIONS] <COMMAND>
```

### オプション

- `--tmp-dir <PATH>`: 中間ファイルの保存先（デフォルト: `./tmp`）
- `--app-id <APP_ID>`: e-Stat API を使うサブコマンド向けの appId（省略時は `ESTAT_APP_ID` を使用）
- `--help`: ヘルプを表示
- `--version`: バージョンを表示

### 出力先・接続先の指定

- `areamap`: `--output` で `ogr2ogr` の出力先データソースを指定します（必須）。
- `areamap`: `--output-format` で `ogr2ogr -f` のドライバ名を指定できます（任意）。
- `areamap`: `--year` で対象年度を1つに絞り込めます（任意）。
- `mesh`: `--postgres-url` で PostgreSQL 接続文字列を指定します（必須）。

`mesh-info` / `mesh-csv` / `mesh-tile` / `db-csv` サブコマンドでは DB 接続は不要です。

例:
```shell
jp-estat-util areamap \
  --output "./output/jp_estat_areamap.gpkg" \
  --output-format GPKG

jp-estat-util mesh \
  --postgres-url "host=127.0.0.1 dbname=jp-estat user=postgres password=mypassword" \
  --level 3 \
  --year 2020 \
  --survey "人口及び世帯"
```

## サブコマンド

### areamap - 小地域（丁目・字等）の取り込み

国勢調査の小地域境界データをダウンロードし、`ogr2ogr` で指定した出力先に書き出します。

#### 概要

- **データソース**: [e-Stat 小地域境界データ](https://www.e-stat.go.jp/gis/statmap-search?page=1&type=2&aggregateUnitForBoundary=A&toukeiCode=00200521)
- **対象年度**: 2000, 2005, 2010, 2015, 2020年
- **座標系**: 既定はJGD2011/JGD2000（`--output-crs` で変更可能）
- **データ形式**: 元データは緯度経度（必要に応じて `--output-crs` で変換可能）

#### 使用方法

```shell
jp-estat-util areamap \
  --output "./output/jp_estat_areamap.gpkg" \
  --output-format GPKG
```

```shell
jp-estat-util areamap \
  --output "PG:host=127.0.0.1 dbname=jp-estat user=postgres password=mypassword" \
  --output-format PostgreSQL
```

```shell
jp-estat-util areamap \
  --output "./output/jp_estat_areamap_2020.gpkg" \
  --output-format GPKG \
  --year 2020
```

```shell
jp-estat-util areamap \
  --output "./output/jp_estat_areamap_2020.geojson" \
  --output-format GeoJSON \
  --output-crs EPSG:4326 \
  --year 2020
```

#### パラメータ

- `--output <OUTPUT>`: `ogr2ogr` に渡す出力先データソース（例: `PG:...`, `./out.gpkg`, `./out.geojson`）
- `--output-format <OUTPUT_FORMAT>`: 出力ドライバ名（例: `PostgreSQL`, `GPKG`, `GeoJSON`）。省略時は `ogr2ogr` の既定/推測に従います。
- `--output-crs <OUTPUT_CRS>`: 出力座標参照系（`ogr2ogr -t_srs` に渡す値。例: `EPSG:4326`）
- `--year <YEAR>`: 対象年度で絞り込み（単年のみ。`2000`, `2005`, `2010`, `2015`, `2020`）

`Parquet` / `GeoJSON` / `FlatGeobuf` / `CSV` などの単一レイヤー形式では、`--year` が必須です。
この場合、出力レイヤー名は出力ファイル名（拡張子除く）に自動調整されます。

#### 処理内容

1. **データダウンロード**: 47都道府県 × 対象年度数（省略時は5年度）を並行ダウンロード
2. **ファイル展開**: ZIPファイルからShapefileを抽出
3. **データ出力**: VRTファイルを作成し、`ogr2ogr` で指定先へ出力
   - 水面調査区（`HCODE=8154`）は `ogr2ogr` の抽出条件で除外
   - `--output-crs` 指定時は `ogr2ogr -t_srs` で座標変換
4. **データ後処理（PostgreSQL出力時のみ）**:
   - メタデータの登録
   - 座標系の設定（既定: JGD2011 SRID 6668 / JGD2000 SRID 4621。`--output-crs` が `EPSG:xxxx` の場合はそのSRIDを使用）

#### PostgreSQL出力時に作成されるテーブル

- `jp_estat_areamap_2000` - 2000年国勢調査小地域境界データ
- `jp_estat_areamap_2005` - 2005年国勢調査小地域境界データ
- `jp_estat_areamap_2010` - 2010年国勢調査小地域境界データ
- `jp_estat_areamap_2015` - 2015年国勢調査小地域境界データ
- `jp_estat_areamap_2020` - 2020年国勢調査小地域境界データ

`--year` を指定した場合は、該当年度のテーブルのみ作成されます。

#### PostgreSQL出力時のテーブル構造

| カラム名 | データ型 | 説明 |
|---------|---------|------|
| `ogc_fid` | integer | 主キー |
| `geom` | geometry(polygon, SRID) | 境界ポリゴン |
| `key_code` | varchar(255) | 小地域コード |
| `pref_name` | varchar(255) | 都道府県名 |
| `city_name` | varchar(255) | 市区町村名 |
| `s_name` | varchar(255) | 小地域名 |
| `jinko` | int | 人口 |
| `setai` | int | 世帯数 |

#### 注意事項

- 処理時間はインターネット接続、メモリ、SSD転送速度に依存
- 途中からの再開機能あり（`--help` で詳細確認）
- ダウンロードしたZIPファイルとShapefileは `./tmp` に保存
- PostgreSQL出力で `--output-crs` を使う場合、`EPSG:xxxx` 形式以外ではメタデータのSRIDが特定できず `geometry(polygon)` として登録されます
- `--output` が PostgreSQL 以外の場合、PostgreSQL向け後処理（メタデータ登録）は実行されません（`HCODE=8154` の除外は出力形式に関係なく実行されます）

---

### mesh - メッシュデータの取り込み

国勢調査のメッシュ統計データをダウンロードし、PostgreSQLに取り込みます。

`mesh-csv` と同じデータ・同じ指定項目（`--level` / `--year` / `--survey`）を使い、出力先だけが異なります（`mesh` は DB 取り込み）。

#### 概要

- **データソース**: e-Stat メッシュ統計データ
- **メッシュレベル**: 3次メッシュ（約1km）、4次メッシュ（約500m）、5次メッシュ（約250m）、6次メッシュ（約125m）
- **対象年度**: 2020年（現在対応）
- **データ形式**: CSV（Shift_JISエンコーディング）

#### 使用方法

```shell
jp-estat-util mesh \
  --postgres-url "host=127.0.0.1 dbname=jp-estat" \
  --level 3 \
  --year 2020 \
  --survey "人口及び世帯"
```

#### パラメータ

- `--postgres-url <POSTGRES_URL>`: PostgreSQL 接続文字列
- `--level <LEVEL>`: メッシュレベル（3, 4, 5, または 6）
- `--year <YEAR>`: 調査年度（例: 2020）
- `--survey <SURVEY>`: 調査名

#### 利用可能なデータ

**2020年データ**:

| レベル | 調査名 | 統計ID | 説明 |
|-------|-------|--------|------|
| 3 | 人口及び世帯 | T001140 | 約1kmメッシュの人口・世帯データ |
| 3 | 人口移動、就業状態等及び従業地・通学地 | T001143 | 約1kmメッシュの移動・就業データ |
| 4 | 人口及び世帯 | T001141 | 約500mメッシュの人口・世帯データ |
| 4 | 人口移動、就業状態等及び従業地・通学地 | T001144 | 約500mメッシュの移動・就業データ |
| 5 | 人口及び世帯 | T001142 | 約250mメッシュの人口・世帯データ |
| 5 | 人口移動、就業状態等及び従業地・通学地 | T001145 | 約250mメッシュの移動・就業データ |
| 6 | 人口及び世帯 | T001231 | 約125mメッシュの人口・世帯データ |
| 6 | 人口移動、就業状態等及び従業地・通学地 | T001232 | 約125mメッシュの移動・就業データ |

#### 処理内容

1. **データ検証**: 指定されたレベル・年度・調査名の組み合わせが利用可能かチェック
2. **データダウンロード**: 全国のメッシュコード（約8,000個）に対応するCSVファイルを並行ダウンロード
3. **スキーマ作成**: CSVヘッダーを解析してテーブルスキーマを自動生成
4. **データ取り込み**: 全CSVファイルをPostgreSQLに取り込み

#### 作成されるテーブル

テーブル名形式: `jp_estat_mesh_{YEAR}_{STATS_ID}_{LEVEL}`

例:
- `jp_estat_mesh_2020_T001140_3` - 2020年3次メッシュ人口・世帯データ

#### データ型の自動判定

| カラム名 | データ型 | 説明 |
|---------|---------|------|
| `KEY_CODE` | BIGINT | メッシュコード |
| `HTKSAKI` | BIGINT | 集計値 |
| `GASSAN` | BIGINT[] | 合算値（配列） |
| `HTKSYORI` | SMALLINT | 処理区分 |
| その他 | INTEGER | 統計値 |

#### メッシュコードについて

`KEY_CODE` フィールドには JIS X 0410 地域メッシュコードが格納されています。このメッシュコードを地理的なジオメトリに変換するには、[jismesh-plpgsql](https://github.com/kotobaMedia/jismesh-plpgsql) ライブラリを使用できます。

このライブラリは、メッシュコードから動的にジオメトリを生成する PL/pgSQL 関数を提供しており、以下のような使い方ができます：

```sql
-- メッシュデータとジオメトリを結合
SELECT
    m."KEY_CODE",
    m."人口（総数）",
    mesh.geom
FROM jp_estat_mesh_2020_T001140_3 m
JOIN jismesh.to_meshcodes(
    ST_SetSRID(ST_MakeBox2D(
        ST_MakePoint(139.7, 35.7),
        ST_MakePoint(135.7, 34.7)
    ), 4326),
    'Lv3'::jismesh.mesh_level
) mesh ON m."KEY_CODE" = mesh.meshcode;
```

#### 使用例

```shell
# 3次メッシュの人口・世帯データを取得
jp-estat-util mesh \
  --postgres-url "host=127.0.0.1 dbname=jp-estat" \
  --level 3 \
  --year 2020 \
  --survey "人口及び世帯"

# 4次メッシュの移動・就業データを取得
jp-estat-util mesh \
  --postgres-url "host=127.0.0.1 dbname=jp-estat" \
  --level 4 \
  --year 2020 \
  --survey "人口移動、就業状態等及び従業地・通学地"
```

#### 注意事項

- メッシュレベルが大きいほどデータ量が増加（5次メッシュは約8,000ファイル）
- CSVファイルはShift_JISエンコーディングで処理
- 空値や `*` は `NULL` として扱われる
- `GASSAN` カラムはセミコロン区切りの配列として保存

---

### 人口メッシュのレベル別 min/max レポート

`misc/population_mesh_minmax.sh` は、`mesh_to_vector.sh` で使う2020年の人口・世帯メッシュについて、全ての数値統計項目の最小値・最大値をメッシュレベルごとにCSV出力します。

- Lv1・Lv2: Lv3データを親メッシュコード単位で合計してから min/max を計算
- Lv3・Lv4・Lv5: 各レベルの元テーブルから min/max を計算
- `KEY_CODE`、`HTKSYORI`、`HTKSAKI` などの識別・制御カラムは対象外
- 統計項目はテーブル定義から自動検出するため、項目名の列挙は不要

```shell
./misc/population_mesh_minmax.sh \
  --output ./output/population_mesh_minmax.csv \
  "host=127.0.0.1 dbname=jp_estat"
```

`--output` を省略するとCSVを標準出力します。テーブルが `public` 以外にある場合は `--schema <SCHEMA>` を指定してください。

出力カラムは `mesh_level,source_table,aggregation,attribute,min,max` です。

---

### mesh-info - 利用可能データ一覧の表示

利用可能なメッシュ統計データを表示します。調査名、年度、メッシュレベル、`stats_id` に加えて、各データセットのバンド（統計項目）名も確認できます。

#### 使用方法

```shell
jp-estat-util mesh-info
```

```shell
jp-estat-util mesh-info --year 2020
```

```shell
jp-estat-util mesh-info --year 2015,2020
```

#### 出力内容

- 調査名ごとの年度一覧・レベル一覧
- 各データセット（`year` / `level` / `stats_id` / `datum`）
- 各データセットのバンド一覧

#### パラメータ

- `--year <YEAR[,YEAR...]>`: 対象年度で絞り込み（例: `--year 2020`, `--year 2015,2020`）

#### 注意事項

- バンド情報の取得時に、各データセットごとに代表CSVを1ファイルダウンロードしてヘッダーを解析します。
- ダウンロードしたZIPと展開ファイルは `--tmp-dir`（既定: `./tmp`）に保存されます。
- ネットワークエラー等でバンド取得に失敗した場合でも、調査名・年度・レベル・`stats_id` の一覧は表示されます。

---

### mesh-csv - メッシュデータのCSV結合出力

メッシュ統計CSVをダウンロードして、1つのCSVに結合して出力します。データベースへの取り込みは行いません。

`mesh` と同じデータ・同じ指定項目（`--level` / `--year` / `--survey`）を使い、出力先だけが異なります（`mesh-csv` は CSV 出力）。

#### 使用方法

```shell
jp-estat-util mesh-csv \
  --level 3 \
  --year 2020 \
  --survey "人口及び世帯" \
  --output ./output/mesh_2020_lv3.csv
```

#### パラメータ

- `--level <LEVEL>`: メッシュレベル（3, 4, 5, または 6）
- `--year <YEAR>`: 調査年度（例: 2020）
- `--survey <SURVEY>`: 調査名
- `--output <OUTPUT>`: 結合CSVの出力先パス

---

### db-csv - 統計表（DB系）の canonical CSV 出力

e-Stat API の `getMetaInfo` / `getStatsData` を使い、DB系の統計表を canonical CSV 群に正規化して出力します。BigQuery への直接アップロード、ファイル系データセット、GIS/Shape データの取得は行いません。

#### 使用方法

```shell
jp-estat-util db-csv \
  --app-id YOUR_APP_ID \
  --output-dir ./output/db_csv \
  --stats-data-id 0003448228 \
  --stats-data-id 0004023604 \
  --raw-json
```

`--app-id` を省略する場合は、事前に `ESTAT_APP_ID` をの環境変数を設定してください。

上記の `statsDataId` は実在する e-Stat の DB 統計表です。

- `0003448228`: [人口推計 / 各年10月1日現在人口 / 令和２年国勢調査基準 / 統計表001 年齢（各歳），男女別人口及び人口性比－総人口，日本人人口](https://www.e-stat.go.jp/dbview?sid=0003448228)
- `0004023604`: [家計調査 / 家計収支編 / 二人以上の世帯 / 品目分類011 品目分類（2025年改定）（総数：数量）](https://www.e-stat.go.jp/dbview?sid=0004023604)

#### パラメータ

- `--app-id <APP_ID>`: e-Stat API の appId（省略時は `ESTAT_APP_ID`）
- `--output-dir <OUTPUT_DIR>`: 出力先ディレクトリ
- `--stats-data-id <STATS_DATA_ID>`: 対象の `statsDataId`（繰り返し指定可）
- `--resume`: 既存の `observations/stats_data_id=<ID>.csv` があるデータセットを再利用
- `--overwrite`: 既存の出力ファイルを上書き
- `--concurrency <N>`: API の同時処理数（既定: `4`）
- `--raw-json`: 生の API JSON を `raw/meta/<ID>.json` と `raw/data/<ID>.json` に保存

#### 出力内容

- `tables.csv`
- `dimensions.csv`
- `dimension_items.csv`
- `observations/stats_data_id=<ID>.csv`
- `manifest.json`
- `raw/meta/<ID>.json`, `raw/data/<ID>.json`（`--raw-json` 指定時）

DuckDB での分析例は [EXAMPLES_DB_CSV.md](./EXAMPLES_DB_CSV.md) を参照してください。

#### 補足

- JSON API のみを利用します。
- 観測値は `time` / `area` / `tab` / `cat01` .. `cat15` の raw code をそのまま保持します。
- `value_text` には API の元値、`value` には数値として扱える値のみを出力します。

#### 必要な `statsDataId` の探し方

`statsDataId` は e-Stat API で指定する「統計表ID」です。e-Stat の API ドキュメントでも、`getMetaInfo` / `getStatsData` の `statsDataId` は「統計表情報取得で得られる統計表ID」として定義されています。

- API ドキュメント: [APIの使い方](https://www.e-stat.go.jp/api/api-dev/how_to_use)
- API 仕様: [政府統計の総合窓口（e-Stat）のAPI仕様](https://www.e-stat.go.jp/api/api-info/e-stat-manual)
- DB 検索画面: [データベース | 統計データを探す](https://www.e-stat.go.jp/stat-search/database?page=1&layout=dataset)

実際には、次の手順で見つけるのが最も簡単です。

1. 上の「データベース」検索画面で、欲しい統計を開きます。
2. テーブル一覧から、欲しい粒度の表を選びます。
3. その行の `DB` または表題リンクを開きます。
4. 開いたページの URL が `https://www.e-stat.go.jp/dbview?sid=0003448228` のような形なら、その `sid` が `--stats-data-id` に渡す値です。

判断のコツ:

- ほしい違いが表題そのものに出ている場合は、別の `statsDataId` を選びます。
- 例: `総世帯` と `二人以上の世帯`、`金額` と `数量`、`令和２年国勢調査基準` と別基準は、通常は別テーブルなので別の `sid` です。
- ほしい違いが表の中の軸に入っている場合は、同じ `statsDataId` のままです。
- 例: `年齢`、`男女`、`都道府県`、`年月`、`品目コード` などは、このコマンドでは `dimensions.csv` / `dimension_items.csv` / `observations/*.csv` に code として出力されます。

---

### mesh-tile - mesh-data-tile 形式でタイル出力

メッシュ統計CSVをダウンロードし、`mesh-data-tile`（`MTI1`）形式の `.tile` ファイル群に変換します。データベースへの取り込みは行いません。

#### 使用方法

```shell
jp-estat-util mesh-tile \
  --level 6 \
  --tile-level 3 \
  --bands 人口（総数）,人口（総数）女,世帯総数 \
  --year 2020 \
  --survey "人口及び世帯" \
  --output-dir ./output/mesh_tiles_lv3_from_lv6
```

#### パラメータ

- `--level <LEVEL>`: 入力データのメッシュレベル（3, 4, 5, または 6）
- `--tile-level <TILE_LEVEL>`: 出力タイルのメッシュレベル（1〜6, `--level` 以下）。省略時は `--level` と同じ
- `--bands <BANDS>`: 出力する統計項目名の並び（カンマ区切り）。省略時は全バンドをCSV順で出力
- `--year <YEAR>`: 調査年度（例: 2020）
- `--survey <SURVEY>`: 調査名
- `--output-dir <OUTPUT_DIR>`: タイル出力先ディレクトリ

#### 出力内容

- `<meshcode>.tile`: JISメッシュコード単位の `mesh-data-tile` バイナリ
- `metadata.json`: バンド定義、`no_data` 値、メッシュレベルなどの付帯情報

#### タイル解像度の考え方

- `--level` は「データの細かさ」、`--tile-level` は「1枚のタイルの大きさ」を表します。
- 例:
  - `--level 3 --tile-level 1`: Lv1タイルにLv3データを格納
  - `--level 4 --tile-level 1`: Lv1タイルにLv4データを格納
  - `--level 6 --tile-level 3`: Lv3タイルにLv6データを格納

#### バンド指定の考え方

- `--bands` を指定すると、その順序がタイル内のバンド順になります。
- 例: `--bands 人口（総数）女,人口（総数）` と指定した場合、band 1 が `人口（総数）女`、band 2 が `人口（総数）` になります。

## ライセンス

このツールは[MITライセンス](./LICENSE)の下で公開されています。

e-Statのデータ利用については、[e-Stat利用規約](https://www.e-stat.go.jp/terms-of-use)に従ってください。
