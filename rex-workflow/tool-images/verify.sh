#!/bin/sh
set -eu

DOCKER=${DOCKER:-docker}
IMAGE_PREFIX=${IMAGE_PREFIX:-rex-tool}
IMAGE_TAG=${IMAGE_TAG:-local}

workspace=$(mktemp -d)
cleanup() {
    rm -rf "$workspace"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
mkdir -p "$workspace/inputs" "$workspace/outputs" "$workspace/scratch"

run_tool() {
    bundle=$1
    entrypoint=$2
    shift 2
    "$DOCKER" run \
        --rm \
        --pull=never \
        --network=none \
        --read-only \
        --cap-drop=ALL \
        --security-opt=no-new-privileges \
        --no-healthcheck \
        --mount "type=bind,source=$workspace,destination=/work" \
        --workdir /work \
        --env MAGICK_TEMPORARY_PATH=/work/scratch \
        --env TMPDIR=/work/scratch \
        --entrypoint "$entrypoint" \
        "${IMAGE_PREFIX}-${bundle}:${IMAGE_TAG}" \
        "$@"
}

run_tool ffmpeg ffmpeg \
    -hide_banner -loglevel error -y \
    -f lavfi -i color=c=blue:s=16x16:r=1 \
    -frames:v 1 /work/outputs/ffmpeg.png
run_tool ffmpeg ffprobe \
    -v error -show_entries format=format_name \
    -of default=noprint_wrappers=1 /work/outputs/ffmpeg.png
test -s "$workspace/outputs/ffmpeg.png"

run_tool imagemagick magick \
    -size 16x16 'canvas:#336699' /work/outputs/imagemagick.webp
run_tool imagemagick magick \
    identify /work/outputs/imagemagick.webp
for command in mogrify compare composite montage; do
    run_tool imagemagick magick "$command" -version >/dev/null
done
run_tool imagemagick magick \
    stream -map rgba -storage-type char \
    /work/outputs/imagemagick.webp /work/outputs/imagemagick.rgba
test -s "$workspace/outputs/imagemagick.webp"
test -s "$workspace/outputs/imagemagick.rgba"

printf '%s\n' \
    '%PDF-1.4' \
    '1 0 obj' \
    '<< /Type /Catalog /Pages 2 0 R >>' \
    'endobj' \
    '2 0 obj' \
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>' \
    'endobj' \
    '3 0 obj' \
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 72 72] /Resources << >> /Contents 4 0 R >>' \
    'endobj' \
    '4 0 obj' \
    '<< /Length 0 >>' \
    'stream' \
    '' \
    'endstream' \
    'endobj' \
    'trailer' \
    '<< /Root 1 0 R /Size 5 >>' \
    '%%EOF' \
    >"$workspace/inputs/source.pdf"

run_tool qpdf qpdf \
    --warning-exit-0 /work/inputs/source.pdf /work/outputs/page.pdf
run_tool qpdf qpdf --check /work/outputs/page.pdf
test -s "$workspace/outputs/page.pdf"

run_tool poppler pdfinfo /work/outputs/page.pdf
run_tool poppler pdftotext \
    /work/outputs/page.pdf /work/outputs/page.txt
run_tool poppler pdftocairo \
    -png -singlefile /work/outputs/page.pdf /work/outputs/page
run_tool poppler pdfimages -list /work/outputs/page.pdf
test -f "$workspace/outputs/page.txt"
test -s "$workspace/outputs/page.png"

echo "All Rex tool images passed their offline, read-only smoke tests."
