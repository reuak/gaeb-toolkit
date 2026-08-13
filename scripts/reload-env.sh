#!/bin/sh
set -eu

cd "$(dirname "$0")/.."

echo "Prüfe Docker-Konfiguration …"
docker compose config --quiet

echo "Übernehme .env und erstelle nur den App-Container neu …"
docker compose up -d --no-deps --force-recreate app

echo "Aktueller Status:"
docker compose ps app

echo "Die letzten App-Meldungen:"
docker compose logs --tail=30 app
