#!/bin/bash
set -euo pipefail
# Creates only synthetic media in the supplied NEW directory, using host FFmpeg.
: "${1:?Usage: generate-fixtures.sh NEW-DIRECTORY}"
mkdir "$1"
cd "$1"
ffmpeg -hide_banner -loglevel error -f lavfi -i sine=frequency=440:sample_rate=48000 -t 20 -metadata title='AppImage music fixture' -metadata artist='Gnune QA' music.wav
ffmpeg -hide_banner -loglevel error -f lavfi -i testsrc2=size=640x360:rate=24 -f lavfi -i sine=frequency=660:sample_rate=48000 -t 20 -c:v libx264 -pix_fmt yuv420p -c:a aac -metadata title='AppImage H264 fixture' video.mp4
ffmpeg -hide_banner -loglevel error -f lavfi -i testsrc2=size=640x360:rate=24 -t 12 -c:v libvpx -b:v 600k -an video.webm
ffmpeg -hide_banner -loglevel error -f lavfi -i testsrc=size=512x512:rate=1 -frames:v 1 cover.png
ffmpeg -hide_banner -loglevel error -f lavfi -i smptebars=size=512x512:rate=1 -frames:v 1 cover.jpg
ffmpeg -hide_banner -loglevel error -i music.wav -i cover.png -map 0:a -map 1:v -c:a flac -c:v copy -disposition:v attached_pic -metadata title='PNG artwork fixture' -metadata album='AppImage Artwork' -metadata artist='Gnune QA' music-png.flac
ffmpeg -hide_banner -loglevel error -i music.wav -i cover.jpg -map 0:a -map 1:v -c:a libmp3lame -c:v copy -disposition:v attached_pic -id3v2_version 3 -metadata title='JPEG artwork fixture' -metadata album='AppImage Artwork JPEG' -metadata artist='Gnune QA' music-jpeg.mp3
