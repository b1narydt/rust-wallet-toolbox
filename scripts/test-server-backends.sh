#!/usr/bin/env bash

set -u
set -o pipefail

if (( $# > 1 )); then
  echo "Usage: $0 [postgres|mysql]" >&2
  exit 2
fi

backend="${1:-}"

case "$backend" in
  "") backend="all" ;;
  postgres | mysql) ;;
  *)
    echo "Usage: $0 [postgres|mysql]" >&2
    exit 2
    ;;
esac

echo "Starting PostgreSQL and MySQL test services..."
if ! docker compose up -d --wait; then
  echo "Failed to start healthy database services." >&2
  echo "When finished, stop them with: docker compose down"
  exit 1
fi

export POSTGRES_DATABASE_URL="postgres://walletuser:walletpass@localhost:55432/wallet_toolbox_test"
export MYSQL_DATABASE_URL="mysql://walletuser:walletpass@localhost:53306/wallet_toolbox_test"

postgres_status=0
mysql_status=0

if [[ "$backend" == "all" || "$backend" == "postgres" ]]; then
  echo "Running PostgreSQL integration suite..."
  cargo test --no-default-features --features postgres --test storage_postgres_tests -- --ignored --test-threads=1 --nocapture
  postgres_status=$?
fi

if [[ "$backend" == "all" || "$backend" == "mysql" ]]; then
  echo "Running MySQL integration suite..."
  cargo test --no-default-features --features mysql --test storage_mysql_tests -- --ignored --test-threads=1 --nocapture
  mysql_status=$?
fi

if [[ "$backend" == "all" || "$backend" == "postgres" ]]; then
  echo "PostgreSQL suite exit code: $postgres_status"
fi
if [[ "$backend" == "all" || "$backend" == "mysql" ]]; then
  echo "MySQL suite exit code: $mysql_status"
fi

echo "Services are still running. Stop them with: docker compose down"

if (( postgres_status != 0 || mysql_status != 0 )); then
  exit 1
fi
