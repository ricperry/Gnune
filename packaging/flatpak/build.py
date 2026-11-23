#!/usr/bin/env python3
"""Build an x86_64 Gnune Flatpak with pinned, source-built media tools.

Requires GNOME SDK/Platform 50, host Rust stable, Python 3, Flatpak and a
Cargo cache containing Cargo.lock dependencies. Does not install the app.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tomllib
import urllib.request

APP_ID = 'org.gnune.Player'
BRANCH = '50'
RECIPE = Path(__file__).resolve().parent


def digest(path):
    return hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, default=RECIPE.parents[1])
    p.add_argument('--work', type=Path, required=True, help='New or resumable build-owned directory')
    p.add_argument('--jobs', type=int, default=6)
    p.add_argument('--skip-native', action='store_true')
    p.add_argument('--skip-rust', action='store_true')
    a = p.parse_args()
    source, work = a.source.resolve(), a.work.resolve()
    version = tomllib.loads((source / 'Cargo.toml').read_text())['package']['version']
    if os.uname().machine != 'x86_64' or a.jobs < 1:
        p.error('x86_64 and positive --jobs required')
    work.mkdir(parents=True, exist_ok=True)
    runtime = work / 'build-runtime'
    runtime.mkdir(mode=0o700, exist_ok=True)
    env = os.environ | {'XDG_RUNTIME_DIR': str(runtime)}
    sdk, stage = work / 'sdk', work / 'source'
    logs = work / 'logs'
    logs.mkdir(exist_ok=True)

    def run(*args, log=None, cwd=source):
        command = list(map(str, args))
        print('+', ' '.join(command), flush=True)
        if log:
            (logs / (log + '.command.json')).write_text(json.dumps(command, indent=2) + '\n')
            with (logs / (log + '.log')).open('w') as f:
                r = subprocess.run(command, cwd=cwd, env=env, stdout=f, stderr=subprocess.STDOUT)
            if r.returncode:
                print((logs / (log + '.log')).read_text()[-12000:], flush=True)
                raise SystemExit('Failed: ' + log)
        else:
            subprocess.run(command, cwd=cwd, env=env, check=True)

    if stage != source:
        for name in ['src', 'assets', 'data', 'packaging', 'scripts', 'tests', 'Cargo.toml', 'Cargo.lock', 'build.rs', 'LICENSE', 'README.md', 'RELEASE_NOTES.md']:
            s, d = source / name, stage / name
            if name == 'assets':
                d.mkdir(parents=True, exist_ok=True)
                for asset in ['gnune.gresource.xml', 'gnuneIconDraft2.png', 'gnuneIconDraft2.svg', 'icons/gnune-rewind-10-symbolic.svg']:
                    (d / asset).parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(s / asset, d / asset)
            elif s.is_dir(): shutil.copytree(s, d, dirs_exist_ok=True, ignore=shutil.ignore_patterns('__pycache__', '*.pyc'))
            elif s.exists(): d.parent.mkdir(parents=True, exist_ok=True); shutil.copy2(s, d)
    if not (stage / 'vendor').exists():
        stage.mkdir(exist_ok=True)
        with (work / 'vendor-config.toml').open('w') as f:
            subprocess.run(['cargo', 'vendor', '--locked', '--offline', str(stage / 'vendor')], cwd=stage, stdout=f, check=True)
        (stage / '.cargo').mkdir(exist_ok=True)
        (stage / '.cargo/config.toml').write_text((work / 'vendor-config.toml').read_text().replace(str(stage / 'vendor'), 'vendor'))
    if not (sdk / 'metadata').exists():
        run('flatpak', 'build-init', sdk, APP_ID, 'org.gnome.Sdk', 'org.gnome.Platform', BRANCH)

    def sdk_run(label, args, cwd=None, extra=()):
        run('flatpak', 'build', '--unshare=network', f'--filesystem={work}',
            f'--filesystem={source}:ro', f'--build-dir={cwd or work}',
            '--env=PKG_CONFIG_PATH=/app/lib/pkgconfig', '--env=LD_LIBRARY_PATH=/app/lib',
            *extra, sdk, *args, log=label)

    downloads, native = work / 'downloads', work / 'native'
    downloads.mkdir(exist_ok=True); native.mkdir(exist_ok=True)
    sources = json.loads((RECIPE / 'sources.json').read_text())
    for item in sources:
        f = downloads / item['name']
        if not f.exists(): urllib.request.urlretrieve(item['url'], f)
        if digest(f) != item['sha256']: raise SystemExit('Checksum mismatch: ' + str(f))
        if not a.skip_native:
            with tarfile.open(f) as t: t.extractall(native, filter='data')
    jobs = str(a.jobs)
    if not a.skip_native:
        c = native / 'chromaprint-build'
        sdk_run('chromaprint-configure', ['cmake', '-S', native / 'chromaprint-1.5.1', '-B', c,
                '-DCMAKE_POLICY_VERSION_MINIMUM=3.5', '-DCMAKE_INSTALL_PREFIX=/app',
                '-DCMAKE_INSTALL_LIBDIR=lib', '-DCMAKE_BUILD_TYPE=Release', '-DFFT_LIB=kissfft',
                '-DBUILD_SHARED_LIBS=ON', '-DBUILD_TOOLS=OFF', '-DBUILD_TESTS=OFF'])
        sdk_run('chromaprint-build', ['cmake', '--build', c, '--parallel', jobs])
        sdk_run('chromaprint-install', ['cmake', '--install', c])
        l = native / 'libass-build'; l.mkdir(exist_ok=True)
        sdk_run('libass-configure', [native / 'libass-0.17.3/configure', '--prefix=/app',
                '--libdir=/app/lib', '--disable-static', '--disable-require-system-font-provider'], cwd=l)
        sdk_run('libass-build', ['make', '-j', jobs], cwd=l)
        sdk_run('libass-install', ['make', 'install'], cwd=l)
        l = native / 'libplacebo-build'
        sdk_run('libplacebo-configure', ['meson', 'setup', l, native / 'libplacebo-7.351.0',
                '--prefix=/app', '--libdir=lib', '--buildtype=release', '--auto-features=disabled',
                '-Ddemos=false', '-Dtests=false'])
        sdk_run('libplacebo-build', ['ninja', '-C', l, '-j', jobs])
        sdk_run('libplacebo-install', ['meson', 'install', '-C', l])
        f = native / 'ffmpeg-build'; f.mkdir(exist_ok=True)
        sdk_run('ffmpeg-configure', [native / 'ffmpeg-8.1.2/configure', '--prefix=/app',
                '--libdir=/app/lib', '--enable-shared', '--disable-static', '--disable-autodetect',
                '--disable-gpl', '--disable-nonfree', '--disable-version3', '--disable-doc',
                '--disable-debug', '--disable-ffplay', '--disable-network', '--disable-hwaccels',
                '--disable-devices', '--enable-indev=lavfi', '--enable-protocol=file,pipe',
                '--enable-chromaprint', '--enable-pthreads', '--enable-zlib', '--enable-bzlib',
                '--enable-lzma', '--extra-ldflags=-Wl,-rpath,/app/lib'], cwd=f)
        sdk_run('ffmpeg-build', ['make', '-j', jobs], cwd=f)
        sdk_run('ffmpeg-install', ['make', 'install'], cwd=f)
        m = native / 'mpv-0.41-build'
        sdk_run('mpv-configure', ['meson', 'setup', m, native / 'mpv-0.41.0', '--prefix=/app',
                '--libdir=lib', '--buildtype=release', '--auto-features=disabled',
                '-Dcplayer=true', '-Dlibmpv=false', '-Dgl=disabled', '-Dgpl=true', '-Dbuild-date=false',
                '-Dpulse=enabled', '-Dlibarchive=enabled', '-Dlua=disabled', '-Dmanpage-build=disabled'])
        sdk_run('mpv-build', ['ninja', '-C', m, '-j', jobs])
        sdk_run('mpv-install', ['meson', 'install', '-C', m])
        g = native / 'gst-libav-build'
        sdk_run('gst-libav-configure', ['meson', 'setup', g, native / 'gst-libav-1.26.11',
                '--prefix=/app', '--libdir=lib', '-Dtests=disabled', '-Ddoc=disabled',
                '--wrap-mode=nodownload'])
        sdk_run('gst-libav-build', ['ninja', '-C', g, '-j', jobs])
        sdk_run('gst-libav-install', ['ninja', '-C', g, 'install'])
    if not a.skip_rust:
        toolchain = Path(subprocess.check_output(['rustup', 'which', 'cargo'], text=True).strip()).parent
        sdk_run('rust-release', [toolchain / 'cargo', 'build', '--manifest-path', stage / 'Cargo.toml',
                '--release', '--locked', '--offline', '-j', jobs], cwd=stage,
                extra=[f'--filesystem={toolchain.parent}:ro', f'--env=PATH={toolchain}:/app/bin:/usr/bin',
                       f'--env=CARGO_HOME={work}/cargo-home', f'--env=CARGO_TARGET_DIR={work}/cargo'])
    prefix = sdk / 'files'
    def install(s, d):
        d.parent.mkdir(parents=True, exist_ok=True); shutil.copy2(s, d)
    install(work / 'cargo/release/gnune', prefix / 'bin/gnune')
    sdk_run('strip-gnune', ['strip', '--strip-unneeded', '/app/bin/gnune'])
    for suffix, folder in [('.desktop', 'applications'), ('.metainfo.xml', 'metainfo')]:
        install(stage / 'data' / (APP_ID + suffix), prefix / 'share' / folder / (APP_ID + suffix))
    # Reuse the checked-in square export of the author's current mark.
    icon = prefix / 'share/icons/hicolor/512x512/apps' / (APP_ID + '.png')
    install(stage / 'src/mobile/icon-512.png', icon)
    scalable = prefix / 'share/icons/hicolor/scalable/apps' / (APP_ID + '.svg')
    if scalable.exists(): scalable.unlink()
    notices = prefix / 'share/licenses/gnune'
    install(stage / 'LICENSE', notices / 'LICENSE')
    install(RECIPE / 'THIRD_PARTY.md', notices / 'THIRD_PARTY.md')
    install(source / 'packaging/licenses/RUST-DEPENDENCIES.md', notices / 'RUST-DEPENDENCIES.md')
    install(source / 'packaging/licenses/RUST-DEPENDENCIES.md', stage / 'packaging/licenses/RUST-DEPENDENCIES.md')
    # Exact source accompanies every bundled native library/executable and every Rust crate.
    src = prefix / 'share/gnune/sources'; src.mkdir(parents=True, exist_ok=True)
    for item in sources: install(downloads / item['name'], src / item['name'])
    app_source = src / f'gnune-{version}-source.tar.xz'
    with tarfile.open(app_source, 'w:xz') as t:
        for child in sorted(stage.iterdir()): t.add(child, arcname=f'gnune-{version}/' + child.name)
    # Headers, pkgconfig and archives serve no runtime purpose; retain source above.
    for child in ['include', 'lib/pkgconfig', 'share/man']:
        d = prefix / child
        if d.exists(): shutil.rmtree(d)
    for f in (prefix / 'lib').glob('*.a'): f.unlink()
    for folder in ['applications', 'icons']:
        for f in (prefix / 'share' / folder).rglob('mpv*'):
            if f.is_file(): f.unlink()
    for item in sources:
        installed = src / item['name']
        if digest(installed) != item['sha256']:
            raise SystemExit('Installed native source changed: ' + item['name'])
    record = {'app_id': APP_ID, 'version': version, 'runtime': 'org.gnome.Platform/x86_64/50',
              'sdk_commit': subprocess.check_output(['flatpak', 'info', '--show-commit', 'org.gnome.Sdk//50'],text=True).strip(),
              'runtime_commit': subprocess.check_output(['flatpak', 'info', '--show-commit', 'org.gnome.Platform//50'],text=True).strip(),
              'sources': sources, 'files': {str(f.relative_to(stage)):digest(f) for f in sorted(stage.rglob('*')) if f.is_file()}}
    (work / 'build-info.json').write_text(json.dumps(record,indent=2)+'\n')
    install(work / 'build-info.json', prefix / 'share/gnune/build-info.json')
    # Recreate only generated desktop exports when resuming this build-owned tree.
    if (sdk / 'export').exists(): shutil.rmtree(sdk / 'export')
    run('flatpak', 'build-finish', '--command=gnune', '--socket=wayland', '--socket=fallback-x11',
        '--socket=pulseaudio', '--env=GST_PLUGIN_PATH=/app/lib/gstreamer-1.0', '--share=ipc', '--share=network', '--device=dri',
        '--filesystem=xdg-music:ro', '--filesystem=xdg-videos:ro', '--filesystem=xdg-pictures:ro',
        '--own-name=org.mpris.MediaPlayer2.Gnune', '--system-talk-name=org.bluez', sdk)
    repo = work / 'repo'
    run('flatpak', 'build-export', repo, sdk, 'stable')
    dist = work / 'dist'; dist.mkdir(exist_ok=True)
    bundle = dist / f'Gnune-{version}-x86_64.flatpak'
    run('flatpak', 'build-bundle', repo, bundle, APP_ID, 'stable', '--runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo')
    (dist / 'SHA256SUMS').write_text(digest(bundle) + '  ' + bundle.name + '\n')
    shutil.copy2(work / 'build-info.json', dist / 'flatpak-build-info.json')
    print(bundle)

if __name__ == '__main__': main()
