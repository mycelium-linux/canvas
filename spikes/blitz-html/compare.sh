#!/usr/bin/env bash
# Render out/<key>.html with WeasyPrint the way doc-forge does, rasterise it, and
# report the PDF properties that a raster preview structurally cannot have.
set -u
cd "$(dirname "$0")"
KEY="${1:-blitz}"
HTML="out/$KEY.html"
[ -f "$HTML" ] || { echo "no $HTML - run the binary first"; exit 1; }

weasyprint "$HTML" "out/$KEY.weasy.pdf" > "out/$KEY.weasy.log" 2>&1; rc=$?
echo "weasyprint exit: $rc"
[ $rc -eq 0 ] || { cat "out/$KEY.weasy.log"; exit $rc; }

echo "== WEASYPRINT: $KEY =="
pdfinfo "out/$KEY.weasy.pdf" | grep -iE "^(Pages|Page size)"
echo "-- embedded fonts --"; pdffonts "out/$KEY.weasy.pdf"
CHARS=$(pdftotext "out/$KEY.weasy.pdf" - | wc -c | tr -d ' ')
echo "-- extractable text: $CHARS chars (a raster gives 0) --"
pdftotext "out/$KEY.weasy.pdf" - | grep -v '^$' | head -12

pdftoppm -png -r 384 -f 1 -l 1 -singlefile "out/$KEY.weasy.pdf" "out/$KEY.weasy"
echo "wrote out/$KEY.weasy.png ($(python3 -c "
import struct;d=open('out/$KEY.weasy.png','rb').read();print('x'.join(map(str,struct.unpack('>II',d[16:24]))))"))"
echo "blitz.png       ($(python3 -c "
import struct;d=open('out/$KEY.png','rb').read();print('x'.join(map(str,struct.unpack('>II',d[16:24]))))"))"
