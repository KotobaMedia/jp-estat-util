#!/usr/bin/env bash

set -euo pipefail

usage() {
    cat <<'EOF'
Calculate the minimum and maximum of every population-mesh statistic at Lv1-Lv5.

Usage:
  population_mesh_minmax.sh [--schema SCHEMA] [--output FILE] "<PG connection string>"

Options:
  --schema SCHEMA  Schema containing the mesh tables (default: public)
  --output FILE    Write CSV to FILE instead of stdout
  -h, --help       Show this help

Lv1 and Lv2 are calculated by summing Lv3 cells into their parent mesh.
Lv3, Lv4, and Lv5 use their native 2020 population-and-household tables.

Example:
  ./misc/population_mesh_minmax.sh \
    --output ./output/population_mesh_minmax.csv \
    "host=127.0.0.1 dbname=jp_estat"
EOF
}

schema_name=public
output_file=-
pg_connection_string=

while (($# > 0)); do
    case "$1" in
        --schema)
            if (($# < 2)); then
                echo "error: --schema requires a value" >&2
                usage >&2
                exit 2
            fi
            schema_name=$2
            shift 2
            ;;
        --output)
            if (($# < 2)); then
                echo "error: --output requires a value" >&2
                usage >&2
                exit 2
            fi
            output_file=$2
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            break
            ;;
        -*)
            echo "error: unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
        *)
            if [[ -n "$pg_connection_string" ]]; then
                echo "error: only one PostgreSQL connection string may be specified" >&2
                usage >&2
                exit 2
            fi
            pg_connection_string=$1
            shift
            ;;
    esac
done

if (($# > 0)); then
    if [[ -n "$pg_connection_string" || $# > 1 ]]; then
        echo "error: only one PostgreSQL connection string may be specified" >&2
        usage >&2
        exit 2
    fi
    pg_connection_string=$1
fi

if [[ -z "$pg_connection_string" ]]; then
    echo "error: a PostgreSQL connection string is required" >&2
    usage >&2
    exit 2
fi

if ! command -v psql >/dev/null 2>&1; then
    echo "error: psql is required but was not found in PATH" >&2
    exit 1
fi

run_report() {
    psql \
        --no-psqlrc \
        --quiet \
        --set=ON_ERROR_STOP=1 \
        --set=schema_name="$schema_name" \
        "$pg_connection_string" <<'SQL'
CREATE TEMPORARY TABLE population_mesh_level_spec (
    mesh_level smallint PRIMARY KEY,
    source_table text NOT NULL,
    prefix_length smallint,
    aggregation text NOT NULL
);

INSERT INTO population_mesh_level_spec
    (mesh_level, source_table, prefix_length, aggregation)
VALUES
    (1, 'jp_estat_mesh_2020_t001140_3', 4, 'sum_by_parent'),
    (2, 'jp_estat_mesh_2020_t001140_3', 6, 'sum_by_parent'),
    (3, 'jp_estat_mesh_2020_t001140_3', NULL, 'native'),
    (4, 'jp_estat_mesh_2020_t001141_4', NULL, 'native'),
    (5, 'jp_estat_mesh_2020_t001142_5', NULL, 'native');

CREATE TEMPORARY TABLE population_mesh_settings (
    schema_name text NOT NULL
);

INSERT INTO population_mesh_settings (schema_name)
VALUES (:'schema_name');

CREATE TEMPORARY TABLE population_mesh_minmax (
    mesh_level smallint NOT NULL,
    source_table text NOT NULL,
    aggregation text NOT NULL,
    attribute_order smallint NOT NULL,
    attribute text NOT NULL,
    min_value numeric,
    max_value numeric
);

DO $do$
DECLARE
    configured_schema text := (
        SELECT schema_name
        FROM population_mesh_settings
    );
    spec record;
    stat_column record;
    source_relation regclass;
    attribute_count integer;
    value_alias text;
    min_alias text;
    max_alias text;
    value_expressions text;
    summary_expressions text;
    range_rows text;
BEGIN
    FOR spec IN
        SELECT *
        FROM population_mesh_level_spec
        ORDER BY mesh_level
    LOOP
        source_relation := to_regclass(
            format('%I.%I', configured_schema, spec.source_table)
        );
        IF source_relation IS NULL THEN
            RAISE EXCEPTION 'required mesh table %.% does not exist',
                configured_schema, spec.source_table;
        END IF;

        attribute_count := 0;
        value_expressions := NULL;
        summary_expressions := NULL;
        range_rows := NULL;

        FOR stat_column IN
            SELECT
                attribute.attname AS attribute_name,
                attribute.attnum AS attribute_order
            FROM pg_attribute AS attribute
            JOIN pg_type AS data_type
              ON data_type.oid = attribute.atttypid
            WHERE attribute.attrelid = source_relation
              AND attribute.attnum > 0
              AND NOT attribute.attisdropped
              AND data_type.typcategory = 'N'
              AND upper(attribute.attname) NOT IN (
                  'KEY_CODE',
                  'HTKSYORI',
                  'HTKSAKI',
                  'LEVEL',
                  'OGC_FID'
              )
            ORDER BY attribute.attnum
        LOOP
            attribute_count := attribute_count + 1;
            value_alias := format('value_%s', stat_column.attribute_order);
            min_alias := format('min_%s', stat_column.attribute_order);
            max_alias := format('max_%s', stat_column.attribute_order);

            IF spec.aggregation = 'sum_by_parent' THEN
                value_expressions := concat_ws(
                    ', ',
                    value_expressions,
                    format(
                        'sum(%I)::numeric AS %I',
                        stat_column.attribute_name,
                        value_alias
                    )
                );
                summary_expressions := concat_ws(
                    ', ',
                    summary_expressions,
                    format(
                        'min(%I)::numeric AS %I, max(%I)::numeric AS %I',
                        value_alias,
                        min_alias,
                        value_alias,
                        max_alias
                    )
                );
            ELSIF spec.aggregation = 'native' THEN
                summary_expressions := concat_ws(
                    ', ',
                    summary_expressions,
                    format(
                        'min(%I)::numeric AS %I, max(%I)::numeric AS %I',
                        stat_column.attribute_name,
                        min_alias,
                        stat_column.attribute_name,
                        max_alias
                    )
                );
            ELSE
                RAISE EXCEPTION 'unsupported aggregation: %', spec.aggregation;
            END IF;

            range_rows := concat_ws(
                ', ',
                range_rows,
                format(
                    '(%s, %L, summary.%I, summary.%I)',
                    stat_column.attribute_order,
                    stat_column.attribute_name,
                    min_alias,
                    max_alias
                )
            );
        END LOOP;

        IF attribute_count = 0 THEN
            RAISE EXCEPTION 'no numeric statistical attributes found in %.%',
                configured_schema, spec.source_table;
        END IF;

        IF spec.aggregation = 'sum_by_parent' THEN
            EXECUTE format(
                'INSERT INTO population_mesh_minmax
                    (mesh_level, source_table, aggregation, attribute_order,
                     attribute, min_value, max_value)
                 SELECT %L, %L, %L,
                        ranges.attribute_order,
                        ranges.attribute,
                        ranges.min_value,
                        ranges.max_value
                 FROM (
                     SELECT %s
                     FROM (
                         SELECT %s
                         FROM %I.%I
                         GROUP BY left("KEY_CODE"::text, %s)
                     ) AS overview_values
                 ) AS summary
                 CROSS JOIN LATERAL (
                     VALUES %s
                 ) AS ranges(attribute_order, attribute, min_value, max_value)',
                spec.mesh_level,
                spec.source_table,
                spec.aggregation,
                summary_expressions,
                value_expressions,
                configured_schema,
                spec.source_table,
                spec.prefix_length,
                range_rows
            );
        ELSE
            EXECUTE format(
                'INSERT INTO population_mesh_minmax
                    (mesh_level, source_table, aggregation, attribute_order,
                     attribute, min_value, max_value)
                 SELECT %L, %L, %L,
                        ranges.attribute_order,
                        ranges.attribute,
                        ranges.min_value,
                        ranges.max_value
                 FROM (
                     SELECT %s
                     FROM %I.%I
                 ) AS summary
                 CROSS JOIN LATERAL (
                     VALUES %s
                 ) AS ranges(attribute_order, attribute, min_value, max_value)',
                spec.mesh_level,
                spec.source_table,
                spec.aggregation,
                summary_expressions,
                configured_schema,
                spec.source_table,
                range_rows
            );
        END IF;
    END LOOP;
END
$do$;

COPY (
    SELECT
        mesh_level,
        source_table,
        aggregation,
        attribute,
        min_value AS min,
        max_value AS max
    FROM population_mesh_minmax
    ORDER BY mesh_level, attribute_order
) TO STDOUT WITH (FORMAT CSV, HEADER true);
SQL
}

if [[ "$output_file" == "-" ]]; then
    run_report
    exit 0
fi

output_dir=$(dirname -- "$output_file")
mkdir -p -- "$output_dir"
temporary_output=$(mktemp "${output_file}.tmp.XXXXXX")
trap 'rm -f -- "$temporary_output"' EXIT

run_report >"$temporary_output"
mv -- "$temporary_output" "$output_file"
trap - EXIT

echo "Population mesh min/max CSV written to $output_file" >&2
