# GAEB.hawkvision.de REST-API v1

Stand: 13. August 2026
Status: Testversion – noch nicht als produktive Enterprise-API freigegeben

Die REST-API konvertiert PDF- und GAEB-Dateien serverseitig. Ein angebundenes
System wie Dolibarr muss die verschiedenen GAEB-Formate nicht selbst parsen.

Typischer Dolibarr-Ablauf:

```text
P83 in Dolibarr hochladen
        ↓
POST /api/v1/convert, format=x83
        ↓
X83 als HTTP-Antwort
        ↓
X83 im Dolibarr-GAEB-Modul importieren
```

## Basis-URL

Test beziehungsweise Produktion nach der Veröffentlichung:

```text
https://gaeb.hawk-vision.de/api/v1
```

Alle Aufrufe müssen über HTTPS erfolgen.

## Authentifizierung

Der API-Key wird bevorzugt als Bearer-Token übertragen:

```http
Authorization: Bearer <API_KEY>
```

Alternativ unterstützt die API:

```http
X-API-Key: <API_KEY>
```

Der Token darf nur auf dem Dolibarr-Server gespeichert werden. Er darf weder in
JavaScript noch in einer öffentlich ausgelieferten HTML-Seite stehen.

## Formate und API-Status abfragen

```http
GET /api/v1/formats
```

Beispiel:

```bash
curl -sS https://gaeb.hawk-vision.de/api/v1/formats \
  -H "Authorization: Bearer $GAEB_API_KEY"
```

Beispielantwort:

```json
{
  "name": "GAEB.hawkvision.de Conversion API",
  "version": "v1",
  "max_upload_bytes": 26214400,
  "authentication": "Authorization: Bearer <key> oder X-API-Key: <key>",
  "input_formats": [
    "pdf", "d81", "d83", "d84", "p81", "p83", "p84",
    "x80", "x81", "x82", "x83", "x84", "x85", "x86", "x89", "xml"
  ],
  "output_formats": ["pdf", "x83", "x84", "p84", "d84"],
  "multiple_outputs": "Mehrere Formate werden als ZIP geliefert."
}
```

## Datei konvertieren

```http
POST /api/v1/convert
Content-Type: multipart/form-data
```

### Multipart-Felder

| Feld | Pflicht | Beschreibung |
|---|---:|---|
| `file` | ja | Zu konvertierende PDF- oder GAEB-Datei |
| `format` | ja* | Einzelnes Zielformat |
| `formats` | ja* | Kommaseparierte Zielformate |
| `allow_conflicts` | nein | `false` als Standard; `true` nur für ausdrücklich prüfpflichtige interne Exporte |

\* `format` oder `formats` muss vorhanden sein.

### P83 nach X83 für Dolibarr

```bash
curl -fS https://gaeb.hawk-vision.de/api/v1/convert \
  -H "Authorization: Bearer $GAEB_API_KEY" \
  -F "file=@/pfad/ausschreibung.p83" \
  -F "format=x83" \
  -o ausschreibung.x83
```

Erfolgreiche Antwort:

```http
HTTP/1.1 200 OK
Content-Type: application/xml; charset=utf-8
Content-Disposition: attachment; filename="ausschreibung.x83"
```

Der Response-Body ist unmittelbar die erzeugte X83-Datei.

### GAEB nach PDF

```bash
curl -fS https://gaeb.hawk-vision.de/api/v1/convert \
  -H "Authorization: Bearer $GAEB_API_KEY" \
  -F "file=@angebot.x84" \
  -F "format=pdf" \
  -o angebot.pdf
```

### Mehrere Ausgaben als ZIP

```bash
curl -fS https://gaeb.hawk-vision.de/api/v1/convert \
  -H "Authorization: Bearer $GAEB_API_KEY" \
  -F "file=@angebot.x84" \
  -F "formats=pdf,p84,x84" \
  -o angebot-exporte.zip
```

Erfolgreiche Antwort:

```http
HTTP/1.1 200 OK
Content-Type: application/zip
Content-Disposition: attachment; filename="angebot-exporte.zip"
```

## Unterstützte Konvertierungen

| Quelle | PDF | X83 | X84 | P84 | D84 |
|---|:---:|:---:|:---:|:---:|:---:|
| PDF | – | ja | bei vollständigen Preisen | bei vollständigen Preisen | bei vollständigen Preisen und kompatibler OZ |
| D81/D83/D84 | ja | ja | bei vollständigen Preisen | bei vollständigen Preisen | bei vollständigen Preisen und kompatibler OZ |
| P81/P83/P84 | ja | ja | bei vollständigen Preisen | bei vollständigen Preisen | bei vollständigen Preisen und kompatibler OZ |
| X80–X86/X89/XML | ja | ja | bei vollständigen Preisen | bei vollständigen Preisen | bei vollständigen Preisen und kompatibler OZ |

Hinweise:

- Eine P83→X83-Konvertierung überträgt die vom Parser unterstützte LV-Struktur,
  OZ, Mengen, Einheiten sowie Kurz- und Langtexte.
- X84, P84 und D84 sind Angebotsabgaben und benötigen vollständige Preise.
- Die erzeugte X84 ist ein kompakter Angebotsrücklauf mit OZ, Einheitspreis und
  Gesamtbetrag; Positionsbeschreibungen werden nicht wiederholt.
- GAEB 90 D84 erlaubt nur eine neunstellige OZ-Maske. Eine längere oder nicht
  kompatible OZ wird nicht gekürzt oder neu nummeriert, sondern mit HTTP 422
  abgewiesen.
- Konvertierte Angebotsdateien müssen vor rechtsverbindlicher Abgabe im
  vorgesehenen AVA- oder Vergabesystem geprüft werden.

## HTTP-Statuscodes

| Status | Bedeutung |
|---:|---|
| `200` | Konvertierung erfolgreich |
| `400` | Anfrage, Datei oder Formatauswahl ungültig |
| `401` | API-Key fehlt oder ist ungültig |
| `413` | Datei überschreitet das konfigurierte Größenlimit |
| `422` | Datei lesbar, gewünschte Konvertierung fachlich nicht möglich |
| `500` | Interner Fehler |
| `503` | Integrations-API auf dem Server nicht konfiguriert |

Fehler werden als JSON zurückgegeben:

```json
{
  "error": "P84 benötigt für jede Position einen Einheits- und Gesamtpreis."
}
```

Dolibarr sollte bei jedem Status ungleich `200` den Response-Body protokollieren
und dem Benutzer die Meldung anzeigen. Das fehlerhafte Dokument sollte nicht
automatisch erneut und unbegrenzt gesendet werden.

## PHP-Beispiel für ein Dolibarr-Modul

```php
<?php

function convertGaebToX83(
    string $baseUrl,
    string $apiToken,
    string $sourcePath,
    string $targetPath
): void {
    if (!is_readable($sourcePath)) {
        throw new RuntimeException('Quelldatei ist nicht lesbar.');
    }

    $curl = curl_init(rtrim($baseUrl, '/') . '/api/v1/convert');
    curl_setopt_array($curl, [
        CURLOPT_POST => true,
        CURLOPT_RETURNTRANSFER => true,
        CURLOPT_CONNECTTIMEOUT => 10,
        CURLOPT_TIMEOUT => 120,
        CURLOPT_HTTPHEADER => [
            'Authorization: Bearer ' . $apiToken,
            'Accept: application/xml, application/json',
        ],
        CURLOPT_POSTFIELDS => [
            'file' => new CURLFile(
                $sourcePath,
                'application/octet-stream',
                basename($sourcePath)
            ),
            'format' => 'x83',
        ],
    ]);

    $body = curl_exec($curl);
    $curlError = curl_error($curl);
    $status = (int) curl_getinfo($curl, CURLINFO_RESPONSE_CODE);
    $contentType = (string) curl_getinfo($curl, CURLINFO_CONTENT_TYPE);
    curl_close($curl);

    if ($body === false) {
        throw new RuntimeException('GAEB-Webdienst nicht erreichbar: ' . $curlError);
    }

    if ($status !== 200) {
        $decoded = json_decode($body, true);
        $message = is_array($decoded) && isset($decoded['error'])
            ? $decoded['error']
            : $body;
        throw new RuntimeException(
            sprintf('GAEB-Konvertierung fehlgeschlagen (HTTP %d): %s', $status, $message)
        );
    }

    if (!str_contains(strtolower($contentType), 'xml')) {
        throw new RuntimeException('Unerwarteter Antworttyp: ' . $contentType);
    }

    if (file_put_contents($targetPath, $body, LOCK_EX) === false) {
        throw new RuntimeException('X83-Datei konnte nicht gespeichert werden.');
    }
}
```

Empfohlener Ablauf im Dolibarr-Modul:

1. Originaldatei in einem geschützten temporären Verzeichnis speichern.
2. Endung und Dateigröße lokal vorprüfen.
3. Datei über die API konvertieren.
4. HTTP-Status und `Content-Type` prüfen.
5. X83 in einem neuen Dateinamen speichern; Original nicht überschreiben.
6. X83 an den bestehenden Dolibarr-GAEB-Importer übergeben.
7. Temporäre Dateien nach dem Vorgang löschen.
8. Im Fehlerprotokoll keine API-Keys oder vollständigen Dokumentinhalte speichern.

## Serverkonfiguration

Die API wird über die Server-`.env` aktiviert:

```dotenv
INTEGRATION_API_KEYS=MINDESTENS_32_ZEICHEN_LANGER_ZUFAELLIGER_TOKEN
PAID_MAX_UPLOAD_BYTES=26214400
```

Mehrere Tokens werden kommasepariert eingetragen:

```dotenv
INTEGRATION_API_KEYS=TOKEN_DOLIBARR,TOKEN_WEITERER_CLIENT
```

Nach einer Änderung der Server-`.env` muss nur der Anwendungscontainer neu
erstellt werden. Ein erneuter Image-Build ist dafür nicht notwendig:

```bash
./scripts/reload-env.sh
```

Das Skript startet ausschließlich den Dienst `app` neu. Ein bereits vorhandener
nginx-Reverse-Proxy oder der optionale Caddy-Dienst wird nicht verändert.

Ein zufälliger Schlüssel kann erzeugt werden mit:

```bash
openssl rand -hex 32
```

## Testfreigabe vor Produktion

Vor der produktiven Kopplung sollten mindestens diese Fälle gemeinsam geprüft
werden:

- eine typische P83 aus Dolibarr/NovaAVA nach X83
- eine P83 mit mehreren Hierarchiestufen
- Umlaute, Sonderzeichen und mehrzeilige Langtexte
- Nullmengen und Nullpreise
- Bedarfs- und Eventualpositionen
- eine fachlich fehlerhafte Datei und die Dolibarr-Fehleranzeige
- Größenlimit und Timeout
- ungültiger beziehungsweise gesperrter Token
- Import der erzeugten X83 im bestehenden Dolibarr-GAEB-Modul

Die derzeitige Testversion verwendet statische Server-Tokens. Für einen
Enterprise-Betrieb sind als nächste Ausbaustufe kundenspezifische, widerrufbare
Tokens, Rate-Limits, Nutzungskontingente und ein Audit-Protokoll vorgesehen.
