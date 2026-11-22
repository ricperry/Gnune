#!/usr/bin/env python3
"""Bundle SDK50-built Gnune + the Flatpak recipe's media prefix. Never install/publish."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tomllib

APP_ID = 'org.gnune.Player'
SDK = 'org.gnome.Sdk//50'
TOOL_SHA256 = 'ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0'
# Explicit playback/format set, excluding CUDA, AI, capture, authoring and Qt plugins.
PLUGINS = '''coreelements coretracers playback typefindfunctions app gio soup adaptivedemux2
 audioconvert audioresample audiorate audioparsers audiomixer autodetect alsa pulseaudio
 volume equalizer replaygain audiofx audiotestsrc
 gtk4 opengl videoconvertscale videorate videobox videofilter videoparsersbad videotestsrc
 subparse pango assrender overlaycomposition
 libav isomp4 matroska avi asf ogg wavparse flac wavpack vorbis opus opusparse
 mpg123 lame id3demux apetag taglib jpeg png theora vpx dav1d aom
 mpegpsdemux mpegtsdemux dvdspu realmedia flv rtp rtpmanager rtsp tcp udp hls dash'''.split()
SYSTEM = re.compile(r'^(ld-linux|lib(c|m|mvec|dl|pthread|rt|resolv|nss_[\w]+)\.so|lib(GL|EGL|GLX|GLdispatch|OpenGL|vulkan|drm|gbm))')


def run(*args, **kwargs):
    print('+', ' '.join(map(str, args)), flush=True)
    return subprocess.run(list(map(str, args)), check=True, **kwargs)


def out(*args, **kwargs):
    return subprocess.check_output(list(map(str, args)), text=True, **kwargs).strip()


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def copy(source, target):
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--root', type=Path, required=True, help='Frozen source snapshot')
    p.add_argument('--sdk-build', type=Path, required=True, help='flatpak build-init directory')
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--media-prefix', type=Path, required=True)
    p.add_argument('--work', type=Path, required=True, help='New/empty staging directory')
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--appimagetool', type=Path, required=True)
    p.add_argument('--runtime-file', type=Path, required=True)
    args = p.parse_args()
    version = tomllib.loads((args.root / 'Cargo.toml').read_text())['package']['version']
    if out('uname', '-m') != 'x86_64':
        raise SystemExit('Only x86_64 is supported')
    if sha(args.runtime_file) != 'd30a3ba1388ef57be73faabf606e0d326682f48c87f666106a3c8d6b35b58b4f':
        raise SystemExit('Pinned type2-runtime caf24f9 checksum mismatch')
    if sha(args.appimagetool) != TOOL_SHA256:
        raise SystemExit('appimagetool 1.9.1 checksum mismatch')
    sdk = Path(out('flatpak', 'info', '--show-location', SDK)) / 'files'
    args.work.mkdir(parents=True, exist_ok=True)
    appdir = args.work / 'Gnune.AppDir'
    appdir.mkdir()  # Fail rather than replacing somebody else's staging.
    prefix = appdir / 'usr'
    private = prefix / 'lib'
    copy(args.binary, prefix / 'bin/gnune')
    run('strip', '--strip-unneeded', prefix / 'bin/gnune')
    seeds = [str(args.binary)]
    for name in ['mpv', 'ffmpeg', 'ffprobe']:
        source = args.media_prefix / 'bin' / name
        copy(source, prefix / 'bin' / name)
        seeds.append(str(source))
    # /usr curl from the pinned SDK, with host trust roots (never a stale bundled CA set).
    for name in ['curl', 'gst-inspect-1.0', 'gst-launch-1.0', 'bwrap']:
        copy(sdk / 'bin' / name, prefix / 'bin' / name)
        seeds.append('/usr/bin/' + name)
    gst = sdk / 'lib/x86_64-linux-gnu/gstreamer-1.0'
    for name in PLUGINS:
        source = gst / ('libgst' + name + '.so')
        custom = args.media_prefix / 'lib/gstreamer-1.0' / source.name
        if custom.is_file():
            source = custom
        if source.exists():
            copy(source, private / 'gstreamer-1.0' / source.name)
            seeds.append(str(source) if source == custom else '/usr/lib/x86_64-linux-gnu/gstreamer-1.0/' + source.name)
    if not (private / 'gstreamer-1.0/libgstgtk4.so').is_file():
        raise SystemExit('SDK is missing gtk4paintablesink')
    copy(gst / 'gst-plugin-scanner', prefix / 'libexec/gstreamer-1.0/gst-plugin-scanner')
    seeds.append('/usr/lib/x86_64-linux-gnu/gstreamer-1.0/gst-plugin-scanner')
    for name in ['glycin-image-rs', 'glycin-svg']:
        copy(sdk / 'libexec/glycin-loaders/2+' / name, prefix / 'libexec/glycin-loaders/2+' / name)
        copy(sdk / 'share/glycin-loaders/2+/conf.d' / (name + '.conf'), prefix / 'share/glycin-loaders/2+/conf.d' / (name + '.conf'))
        seeds.append('/usr/libexec/glycin-loaders/2+/' + name)
    for name in ['libgiognutls.so', 'libgiolibproxy.so']:
        source = sdk / 'lib/x86_64-linux-gnu/gio/modules' / name
        if source.exists():
            copy(source, private / 'gio/modules' / name)
            seeds.append('/usr/lib/x86_64-linux-gnu/gio/modules/' + name)
    pixbuf = sdk / 'lib/x86_64-linux-gnu/gdk-pixbuf-2.0/2.10.0'
    for source in (pixbuf / 'loaders').glob('*.so'):
        copy(source, private / 'gdk-pixbuf-2.0/2.10.0/loaders' / source.name)
        seeds.append('/usr/lib/x86_64-linux-gnu/gdk-pixbuf-2.0/2.10.0/loaders/' + source.name)
    copy(pixbuf / 'loaders.cache', private / 'gdk-pixbuf-2.0/2.10.0/loaders.cache')
    # libgstsoup resolves this module at runtime rather than linking it.
    copy(sdk / 'lib/x86_64-linux-gnu/libsoup-3.0.so.0', private / 'libsoup-3.0.so.0')
    seeds.append('/usr/lib/x86_64-linux-gnu/libsoup-3.0.so.0')
    # Match the bundled fontconfig parser while retaining host font directories.
    shutil.copytree(sdk / 'etc/fonts', prefix / 'etc/fonts', symlinks=False)
    for name in ['05-flatpak-fontpath.conf', '50-flatpak.conf']:
        (prefix / 'etc/fonts/conf.d' / name).unlink(missing_ok=True)
    sandbox_args = ['flatpak', 'build', '--unshare=network', f'--filesystem={args.work}',
                    f'--filesystem={args.binary.parent}:ro', f'--filesystem={args.media_prefix}:ro',
                    f'--env=LD_LIBRARY_PATH={args.media_prefix}/lib:/usr/lib/x86_64-linux-gnu', str(args.sdk_build)]
    deps = out(*sandbox_args, 'ldd', *seeds)
    (args.work / 'dependencies.txt').write_text(deps + '\n')
    if 'not found' in deps:
        raise SystemExit('Unresolved SDK dependency, see dependencies.txt')
    bundled = {}
    system = set()
    for path in sorted(set(re.findall(r'=> (/\S+)', deps))):
        name = Path(path).name
        if SYSTEM.match(name):
            system.add(name)
            continue
        source = sdk / path.removeprefix('/usr/') if path.startswith('/usr/') else Path(path)
        if path.startswith('/app/'):
            source = args.media_prefix / path.removeprefix('/app/')
        copy(source, private / name)
        bundled[name] = {'sdk_path': path.replace(str(args.media_prefix), '/app'), 'sha256': sha(source)}
    # SDK data include GTK schemas, basic icons and all runtime license notices.
    archive = args.work / 'runtime-data.tar'
    run(*sandbox_args, 'tar', '-chf', archive, '-C', '/usr/share', 'glib-2.0/schemas', 'icons/Adwaita', 'icons/hicolor', 'mime', 'licenses')
    run('tar', '-xf', archive, '-C', prefix / 'share')
    archive.unlink()
    notice = prefix / 'share/licenses/gnune'
    copy(args.root / 'LICENSE', notice / 'LICENSE')
    copy(args.root / 'data/org.gnune.Player.metainfo.xml', prefix / 'share/metainfo/org.gnune.Player.metainfo.xml')
    copy(sdk / 'manifest.json', notice / 'gnome-sdk50-manifest.json')
    for folder in ['share/licenses', 'share/doc']:
        source = args.media_prefix / folder
        if source.exists():
            shutil.copytree(source, prefix / folder, dirs_exist_ok=True)
    copy(args.root / 'packaging/licenses/RUST-DEPENDENCIES.md', notice / 'RUST-DEPENDENCIES.md')
    recipe = Path(__file__).resolve().parent
    (appdir / 'AppRun').write_text((recipe / 'AppRun').read_text().replace('@GNUNE_VERSION@', version))
    (appdir / 'AppRun').chmod(0o755)
    copy(recipe / (APP_ID + '.desktop'), appdir / (APP_ID + '.desktop'))
    copy(recipe / (APP_ID + '.desktop'), prefix / 'share/applications' / (APP_ID + '.desktop'))
    copy(args.root / 'src/mobile/icon-512.png', appdir / (APP_ID + '.png'))
    copy(args.root / 'src/mobile/icon-512.png', prefix / 'share/icons/hicolor/512x512/apps' / (APP_ID + '.png'))
    (appdir / '.DirIcon').symlink_to(APP_ID + '.png')
    symbols = ''
    for path in appdir.rglob('*'):
        if path.is_file() and path.open('rb').read(4) == b'\x7fELF':
            symbols += out('objdump', '-T', path) + '\n'
    versions = sorted(set(re.findall(r'GLIBC_([\d.]+)', symbols)), key=lambda v:tuple(map(int, v.split('.'))))
    record = {'version': version, 'architecture': 'x86_64', 'minimum_glibc': versions[-1],
              'sdk': SDK, 'sdk_commit': out('flatpak', 'info', '--show-commit', SDK),
              'platform_commit': out('flatpak', 'info', '--show-commit', 'org.gnome.Platform//50'),
              'system_libraries': sorted(system), 'bundled_libraries': bundled,
              'gstreamer_plugins': sorted(p.name for p in (private / 'gstreamer-1.0').glob('*.so')),
              'helper_executables': sorted(p.name for p in (prefix / 'bin').iterdir() if p.name != 'gnune'),
              'packaging_recipe_sha256': {p.name: sha(p) for p in sorted(recipe.iterdir()) if p.is_file()},
              'binary_sha256': sha(args.binary), 'appimagetool_sha256': sha(args.appimagetool),
              'runtime_sha256': sha(args.runtime_file),
              'runtime_source': 'https://github.com/AppImage/type2-runtime/commit/caf24f9',
              'gnune_sources_sha256': {str(p.relative_to(args.root)): sha(p) for name in ['src', 'assets', 'data', 'packaging', 'scripts', 'tests'] for p in sorted((args.root / name).rglob('*')) if p.is_file() and '__pycache__' not in p.parts and p.suffix != '.pyc' and p.name not in {'gnuneIconDraft.png', 'gnuneIconDraft.svg'}},
              'source_metadata_sha256': {name: sha(args.root / name) for name in ['Cargo.toml', 'Cargo.lock', 'build.rs', 'LICENSE', 'README.md', 'RELEASE_NOTES.md']}}
    (notice / 'build-provenance.json').write_text(json.dumps(record, indent=2) + '\n')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    run(args.appimagetool, '--no-appstream', '--runtime-file', args.runtime_file, appdir, args.output,
        env=dict(os.environ, ARCH='x86_64', APPIMAGE_EXTRACT_AND_RUN='1'))
    record['artifact_sha256'] = sha(args.output)
    record['artifact'] = args.output.name
    (args.output.parent / 'appimage-build-info.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps({k:record[k] for k in ['artifact', 'artifact_sha256', 'minimum_glibc']}, indent=2))


if __name__ == '__main__':
    main()
