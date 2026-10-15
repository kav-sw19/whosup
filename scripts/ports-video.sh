#!/bin/sh
# The Ports tab video (MP4, 1920x1080, H.264) for posting: renders the scripted demo frame by
# frame, then encodes it. Needs ffmpeg, and the svg2png helper built from docs/svg2png.swift.
set -e
cd "$(dirname "$0")/.."
S=/tmp/whosup-svg2png; [ -f "$S" ] || swiftc -O docs/svg2png.swift -o "$S"
cargo test --release ports_video -- --ignored --quiet
for f in target/ports-video/*.svg; do "$S" "$f" "${f%.svg}.png" 1.5 >/dev/null; done
# an end card: the wordmark and how to get it, held for 3 seconds
"$S" docs/logo/whosup-dark.svg target/ports-video/logo.png 3 >/dev/null
last=$(ls target/ports-video/[0-9]*.png | tail -1)
size=$(magick identify -format "%wx%h" "$last")
magick -size "$size" xc:'#1a1b26' \( target/ports-video/logo.png \) -gravity center -geometry +0-90 -composite \
  -font /System/Library/Fonts/Menlo.ttc -fill '#c0caf5' -pointsize 34 -annotate +0+50 'Close the dev servers your agents left running' \
  -fill '#7aa2f7' -pointsize 40 -annotate +0+130 'brew install kav-sw19/tap/whosup' \
  -fill '#565f89' -pointsize 26 -annotate +0+200 'github.com/kav-sw19/whosup' target/ports-video/end.png
i=$(basename "$last" .png); i=$((10#$i))
for k in $(seq 1 30); do cp target/ports-video/end.png "target/ports-video/$(printf %04d $((i + k))).png"; done
mkdir -p docs/video
ffmpeg -y -loglevel error -framerate 10 -i target/ports-video/%04d.png \
  -vf "scale=1904:-2:flags=lanczos,pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=0x0d0e14,fps=30" \
  -c:v libx264 -profile:v high -pix_fmt yuv420p -crf 16 -preset slow -movflags +faststart \
  docs/video/whosup-ports.mp4
echo "docs/video/whosup-ports.mp4"
