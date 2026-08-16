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
mkdir -p "$workspace/inputs" "$workspace/outputs"

user="$(id -u):$(id -g)"

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
        --pids-limit 512 \
        --user "$user" \
        --mount "type=bind,source=$workspace/inputs,destination=/work/inputs,readonly" \
        --mount "type=bind,source=$workspace/outputs,destination=/work/outputs" \
        --tmpfs /work/tmp:rw,noexec,nosuid,nodev,mode=1777,size=536870912 \
        --workdir /work \
        --env HOME=/work/tmp \
        --env MAGICK_TEMPORARY_PATH=/work/tmp \
        --env TMPDIR=/work/tmp \
        --env LANG=C \
        --env LC_ALL=C \
        --env TZ=UTC \
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

run_tool ffmpeg ffmpeg \
    -hide_banner -loglevel error -y \
    -f lavfi -i color=c=black:s=128x32 \
    -vf 'drawtext=text=Rex:font=DejaVu Sans:fontcolor=white:x=4:y=4' \
    -frames:v 1 /work/outputs/ffmpeg-font.png
test -s "$workspace/outputs/ffmpeg-font.png"

printf '%s\n' \
    'digraph workflow {' \
    '  graph [rankdir="LR"]' \
    '  prepare -> render' \
    '}' \
    >"$workspace/inputs/workflow.dot"
run_tool graphviz dot \
    -Kdot -Tsvg -o /work/outputs/workflow.svg \
    /work/inputs/workflow.dot
test -s "$workspace/outputs/workflow.svg"
grep -q '<svg' "$workspace/outputs/workflow.svg"

printf '%s\n' \
    'set terminal svg size 128,96 font "DejaVu Sans,10"' \
    'set output "/work/outputs/gnuplot.svg"' \
    'plot x*x with lines title "Rex"' \
    >"$workspace/inputs/gnuplot.gp"
run_tool gnuplot gnuplot /work/inputs/gnuplot.gp
test -s "$workspace/outputs/gnuplot.svg"
grep -q '<svg' "$workspace/outputs/gnuplot.svg"

run_tool imagemagick magick \
    -size 16x16 'canvas:#336699' /work/outputs/imagemagick.webp
run_tool imagemagick magick \
    identify /work/outputs/imagemagick.webp
run_tool imagemagick magick \
    -background white -fill black -font DejaVu-Sans label:Rex \
    /work/outputs/imagemagick-font.png
for command in mogrify compare composite montage; do
    run_tool imagemagick magick "$command" -version >/dev/null
done
run_tool imagemagick magick \
    stream -map rgba -storage-type char \
    /work/outputs/imagemagick.webp /work/outputs/imagemagick.rgba
test -s "$workspace/outputs/imagemagick.webp"
test -s "$workspace/outputs/imagemagick.rgba"
test -s "$workspace/outputs/imagemagick-font.png"

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

if run_tool ffmpeg ffmpeg \
    -hide_banner -loglevel error -y \
    -f lavfi -i color=c=red:s=8x8 -frames:v 1 \
    /work/inputs/must-not-write.png >/dev/null 2>&1; then
    echo "tool unexpectedly wrote to the read-only input mount" >&2
    exit 1
fi
test ! -e "$workspace/inputs/must-not-write.png"

if run_tool ffmpeg ffmpeg \
    -hide_banner -loglevel error -y \
    -f lavfi -i color=c=red:s=8x8 -frames:v 1 \
    /must-not-write.png >/dev/null 2>&1; then
    echo "tool unexpectedly wrote to the read-only container root" >&2
    exit 1
fi

echo "All Rex tool images passed their offline, read-only smoke tests."
