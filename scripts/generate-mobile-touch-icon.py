#!/usr/bin/env python3
"""Render the canonical Gnune raster for iOS; requires Pillow (python3-pillow)."""
from pathlib import Path
from PIL import Image, ImageOps

ROOT = Path(__file__).resolve().parents[1]
source = Image.open(ROOT / 'assets/gnuneIconDraft2.png').convert('RGBA')
mark = ImageOps.contain(source, (180, 180), Image.Resampling.LANCZOS)
canvas = Image.new('RGBA', (180, 180), '#111014')
canvas.alpha_composite(mark, ((180 - mark.width) // 2, (180 - mark.height) // 2))
canvas.convert('RGB').save(ROOT / 'src/mobile/gnune-touch-icon-180-v1.png')
