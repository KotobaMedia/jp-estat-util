#!/bin/bash -e

# A quick script to convert the data in PostGIS to a PMTiles vector archive.
# Prerequisites: data in PostGIS, tippecanoe, jismesh in PostGIS, ogr2ogr, tile-join
# This only works on 2020 data right now.
# Make sure all 3,4,5 zoom levels are available in the database.
# Tile zooms by mesh size:
#   Lv1 (80km):  z0-z4
#   Lv2 (10km):  z5-z7
#   Lv3 (1km):   z8-z10
#   Lv4 (500m):  z11
#   Lv5 (250m):  z12

# Usage: ./mesh_to_vector.sh <output_file> "<PG connection string>"
# Example: ./misc/mesh_to_vector.sh ./output/jp_estat_mesh_2020.pmtiles "host=127.0.0.1 dbname=jp_estat"

tmpdir=$(mktemp -d)
# echo "Working in temporary directory: $tmpdir"
trap 'rm -rf "$tmpdir"' EXIT

output_file=$1
pg_connection_string=$2

ogr2ogr -f FlatGeobuf "$tmpdir/output1.fgb" PG:"$pg_connection_string" -sql 'SELECT left(x."KEY_CODE"::text, 4)::bigint as "KEY_CODE", 1 as "level", sum(x."人口（総数）") as "人口（総数）", sum(x."人口（総数）男") as "人口（総数）男", sum(x."人口（総数）女") as "人口（総数）女", sum(x."０～１４歳人口総数") as "０～１４歳人口総数", sum(x."０～１４歳人口男") as "０～１４歳人口男", sum(x."０～１４歳人口女") as "０～１４歳人口女", sum(x."１５歳以上人口総数") as "１５歳以上人口総数", sum(x."１５歳以上人口男") as "１５歳以上人口男", sum(x."１５歳以上人口女") as "１５歳以上人口女", sum(x."１５～６４歳人口総数") as "１５～６４歳人口総数", sum(x."１５～６４歳人口男") as "１５～６４歳人口男", sum(x."１５～６４歳人口女") as "１５～６４歳人口女", sum(x."１８歳以上人口総数") as "１８歳以上人口総数", sum(x."１８歳以上人口男") as "１８歳以上人口男", sum(x."１８歳以上人口女") as "１８歳以上人口女", sum(x."２０歳以上人口総数") as "２０歳以上人口総数", sum(x."２０歳以上人口男") as "２０歳以上人口男", sum(x."２０歳以上人口女") as "２０歳以上人口女", sum(x."６５歳以上人口総数") as "６５歳以上人口総数", sum(x."６５歳以上人口男") as "６５歳以上人口男", sum(x."６５歳以上人口女") as "６５歳以上人口女", sum(x."７５歳以上人口総数") as "７５歳以上人口総数", sum(x."７５歳以上人口男") as "７５歳以上人口男", sum(x."７５歳以上人口女") as "７５歳以上人口女", sum(x."８５歳以上人口総数") as "８５歳以上人口総数", sum(x."８５歳以上人口男") as "８５歳以上人口男", sum(x."８５歳以上人口女") as "８５歳以上人口女", sum(x."９５歳以上人口総数") as "９５歳以上人口総数", sum(x."９５歳以上人口男") as "９５歳以上人口男", sum(x."９５歳以上人口女") as "９５歳以上人口女", sum(x."外国人人口総数") as "外国人人口総数", sum(x."外国人人口男") as "外国人人口男", sum(x."外国人人口女") as "外国人人口女", sum(x."世帯総数") as "世帯総数", sum(x."一般世帯数") as "一般世帯数", sum(x."１人世帯数一般世帯数") as "１人世帯数一般世帯数", sum(x."２人世帯数一般世帯数") as "２人世帯数一般世帯数", sum(x."３人世帯数一般世帯数") as "３人世帯数一般世帯数", sum(x."４人世帯数一般世帯数") as "４人世帯数一般世帯数", sum(x."５人世帯数一般世帯数") as "５人世帯数一般世帯数", sum(x."６人世帯数一般世帯数") as "６人世帯数一般世帯数", sum(x."７人以上世帯数一般世帯数") as "７人以上世帯数一般世帯数", sum(x."親族のみの世帯数一般世帯数") as "親族のみの世帯数一般世帯数", sum(x."核家族世帯数一般世帯数") as "核家族世帯数一般世帯数", sum(x."核家族以外の世帯数一般世帯数") as "核家族以外の世帯数一般世帯数", sum(x."６歳未満世帯員のいる世帯数一般世帯数") as "６歳未満世帯員のいる世帯数一般世帯数", sum(x."６５歳以上世帯員のいる世帯数一般世帯数") as "６５歳以上世帯員のいる世帯数一般世帯数", sum(x."世帯主の年齢が２０～２９歳の１人世帯数一般") as "世帯主の年齢が２０～２９歳の１人世帯数一般", sum(x."高齢単身世帯数一般世帯数") as "高齢単身世帯数一般世帯数", sum(x."高齢夫婦世帯数一般世帯数") as "高齢夫婦世帯数一般世帯数", jismesh.to_meshpoly_geom(left(x."KEY_CODE"::text, 4)::bigint) as "geom" FROM jp_estat_mesh_2020_t001140_3 x GROUP BY left(x."KEY_CODE"::text, 4)::bigint'

ogr2ogr -f FlatGeobuf "$tmpdir/output2.fgb" PG:"$pg_connection_string" -sql 'SELECT left(x."KEY_CODE"::text, 6)::bigint as "KEY_CODE", 2 as "level", sum(x."人口（総数）") as "人口（総数）", sum(x."人口（総数）男") as "人口（総数）男", sum(x."人口（総数）女") as "人口（総数）女", sum(x."０～１４歳人口総数") as "０～１４歳人口総数", sum(x."０～１４歳人口男") as "０～１４歳人口男", sum(x."０～１４歳人口女") as "０～１４歳人口女", sum(x."１５歳以上人口総数") as "１５歳以上人口総数", sum(x."１５歳以上人口男") as "１５歳以上人口男", sum(x."１５歳以上人口女") as "１５歳以上人口女", sum(x."１５～６４歳人口総数") as "１５～６４歳人口総数", sum(x."１５～６４歳人口男") as "１５～６４歳人口男", sum(x."１５～６４歳人口女") as "１５～６４歳人口女", sum(x."１８歳以上人口総数") as "１８歳以上人口総数", sum(x."１８歳以上人口男") as "１８歳以上人口男", sum(x."１８歳以上人口女") as "１８歳以上人口女", sum(x."２０歳以上人口総数") as "２０歳以上人口総数", sum(x."２０歳以上人口男") as "２０歳以上人口男", sum(x."２０歳以上人口女") as "２０歳以上人口女", sum(x."６５歳以上人口総数") as "６５歳以上人口総数", sum(x."６５歳以上人口男") as "６５歳以上人口男", sum(x."６５歳以上人口女") as "６５歳以上人口女", sum(x."７５歳以上人口総数") as "７５歳以上人口総数", sum(x."７５歳以上人口男") as "７５歳以上人口男", sum(x."７５歳以上人口女") as "７５歳以上人口女", sum(x."８５歳以上人口総数") as "８５歳以上人口総数", sum(x."８５歳以上人口男") as "８５歳以上人口男", sum(x."８５歳以上人口女") as "８５歳以上人口女", sum(x."９５歳以上人口総数") as "９５歳以上人口総数", sum(x."９５歳以上人口男") as "９５歳以上人口男", sum(x."９５歳以上人口女") as "９５歳以上人口女", sum(x."外国人人口総数") as "外国人人口総数", sum(x."外国人人口男") as "外国人人口男", sum(x."外国人人口女") as "外国人人口女", sum(x."世帯総数") as "世帯総数", sum(x."一般世帯数") as "一般世帯数", sum(x."１人世帯数一般世帯数") as "１人世帯数一般世帯数", sum(x."２人世帯数一般世帯数") as "２人世帯数一般世帯数", sum(x."３人世帯数一般世帯数") as "３人世帯数一般世帯数", sum(x."４人世帯数一般世帯数") as "４人世帯数一般世帯数", sum(x."５人世帯数一般世帯数") as "５人世帯数一般世帯数", sum(x."６人世帯数一般世帯数") as "６人世帯数一般世帯数", sum(x."７人以上世帯数一般世帯数") as "７人以上世帯数一般世帯数", sum(x."親族のみの世帯数一般世帯数") as "親族のみの世帯数一般世帯数", sum(x."核家族世帯数一般世帯数") as "核家族世帯数一般世帯数", sum(x."核家族以外の世帯数一般世帯数") as "核家族以外の世帯数一般世帯数", sum(x."６歳未満世帯員のいる世帯数一般世帯数") as "６歳未満世帯員のいる世帯数一般世帯数", sum(x."６５歳以上世帯員のいる世帯数一般世帯数") as "６５歳以上世帯員のいる世帯数一般世帯数", sum(x."世帯主の年齢が２０～２９歳の１人世帯数一般") as "世帯主の年齢が２０～２９歳の１人世帯数一般", sum(x."高齢単身世帯数一般世帯数") as "高齢単身世帯数一般世帯数", sum(x."高齢夫婦世帯数一般世帯数") as "高齢夫婦世帯数一般世帯数", jismesh.to_meshpoly_geom(left(x."KEY_CODE"::text, 6)::bigint) as "geom" FROM jp_estat_mesh_2020_t001140_3 x
GROUP BY left(x."KEY_CODE"::text, 6)::bigint'

ogr2ogr -f FlatGeobuf "$tmpdir/output3.fgb" PG:"$pg_connection_string" -mapFieldType IntegerList=String,Integer64List=String -sql "SELECT x.*, 3 as \"level\", jismesh.to_meshpoly_geom(x.\"KEY_CODE\") as \"geom\" FROM jp_estat_mesh_2020_t001140_3 x"

ogr2ogr -f FlatGeobuf "$tmpdir/output4.fgb" PG:"$pg_connection_string" -mapFieldType IntegerList=String,Integer64List=String -sql "SELECT x.*, 4 as \"level\", jismesh.to_meshpoly_geom(x.\"KEY_CODE\") as \"geom\" FROM jp_estat_mesh_2020_t001141_4 x"

ogr2ogr -f FlatGeobuf "$tmpdir/output5.fgb" PG:"$pg_connection_string" -mapFieldType IntegerList=String,Integer64List=String -sql "SELECT x.*, 5 as \"level\", jismesh.to_meshpoly_geom(x.\"KEY_CODE\") as \"geom\" FROM jp_estat_mesh_2020_t001142_5 x"

echo "==> Creating Lv1 (80km) mbtiles..."
tippecanoe -o "$tmpdir/1.mbtiles" \
    -Z0 \
    -z4 \
    --no-simplification-of-shared-nodes \
    -f \
    --layer=jp_estat_mesh_2020 \
    "$tmpdir/output1.fgb"

echo "==> Creating Lv2 (10km) mbtiles..."
tippecanoe -o "$tmpdir/2.mbtiles" \
    -Z5 \
    -z7 \
    --no-simplification-of-shared-nodes \
    -f \
    --layer=jp_estat_mesh_2020 \
    "$tmpdir/output2.fgb"

echo "==> Creating Lv3 (1km) mbtiles..."
tippecanoe -o "$tmpdir/3.mbtiles" \
    -Z8 \
    -z10 \
    --no-simplification-of-shared-nodes \
    --no-tile-size-limit \
    -f \
    --layer=jp_estat_mesh_2020 \
    "$tmpdir/output3.fgb"

echo "==> Creating Lv4 (1/2  Lv3 mesh) (500m) mbtiles..."
tippecanoe -o "$tmpdir/4.mbtiles" \
    -Z11 \
    -z11 \
    --no-simplification-of-shared-nodes \
    --no-tile-size-limit \
    -f \
    --layer=jp_estat_mesh_2020 \
    "$tmpdir/output4.fgb"

echo "==> Creating Lv5 (1/4  Lv3 mesh) (250m) mbtiles..."
tippecanoe -o "$tmpdir/5.mbtiles" \
    -Z12 \
    -z12 \
    --no-simplification-of-shared-nodes \
    --no-tile-size-limit \
    -f \
    --layer=jp_estat_mesh_2020 \
    "$tmpdir/output5.fgb"

echo "==> Joining all levels..."
tile-join -o "$output_file" \
    --no-tile-size-limit \
    --force \
    "$tmpdir/1.mbtiles" \
    "$tmpdir/2.mbtiles" \
    "$tmpdir/3.mbtiles" \
    "$tmpdir/4.mbtiles" \
    "$tmpdir/5.mbtiles"
