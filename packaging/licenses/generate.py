#!/usr/bin/env python3
"""Generate complete crate license-file inventory for the locked Cargo graph.

Run from any directory. Only local Cargo sources and checked-in supplements are
read; network is not used. License expressions remain upstream declarations.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def generate(project, supplements):
    graph = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--offline', '--format-version', '1'],
        cwd=project, text=True))
    packages = sorted((p for p in graph['packages'] if p['source']),
                      key=lambda p: (p['name'], p['version']))
    lock_sha = hashlib.sha256((project / 'Cargo.lock').read_bytes()).hexdigest()
    provenance = json.loads((supplements / 'provenance.json').read_text())
    out = ['# Gnune Rust dependency notices\n\n',
           f'Cargo.lock SHA-256: `{lock_sha}`\n\n',
           'This inventory preserves the declared license expressions and shipped license,\n'
           'copyright, authorship and notice files for all resolved registry packages,\n'
           'including build, test and other-platform dependencies. Expressions containing\n'
           'OR describe upstream alternatives; this document does not relicense dependencies.\n'
           'Per-file notices remain in the companion vendored source archive.\n'
           'Native libraries and external tools have separate notices in each package.\n\n',
           '| Crate | Version | Declared license |\n| --- | --- | --- |\n']
    for p in packages:
        out.append(f'| {p["name"]} | {p["version"]} | {p["license"] or "UNDECLARED"} |\n')
    for p in packages:
        root = Path(p['manifest_path']).parent
        files = {}
        for path in root.iterdir():
            if path.name.lower().startswith(('license','licence','copying','copyright','notice','authors')):
                paths = [path] if path.is_file() else sorted(x for x in path.rglob('*') if x.is_file())
                for x in paths: files[str(x.relative_to(root))] = x
        if p.get('license_file'):
            x = root / p['license_file']; files[p['license_file']] = x
        crate = f'{p["name"]}-{p["version"]}'
        supplemental = [x for x in provenance if x['crate'] == crate]
        for entry in supplemental:
            x = supplements / 'upstream' / crate / entry['file']
            if hashlib.sha256(x.read_bytes()).hexdigest() != entry['sha256']:
                raise RuntimeError(f'Upstream license checksum mismatch: {x}')
            # Include exact release supplements, even when AUTHORS was in the crate.
            files['upstream/' + entry['file']] = x
        if not files: raise RuntimeError(f'No license or notice files: {crate}')
        out.extend([f'\n## {p["name"]} {p["version"]}\n\n',
                    f'Declared license: `{p["license"]}`\n\n',
                    f'Source: https://crates.io/crates/{p["name"]}/{p["version"]}\n\n'])
        if p.get('repository'):out.append(f'Upstream: {p["repository"]}\n\n')
        for entry in supplemental:
            out.append(f'Supplement: {entry["url"]} (SHA-256 `{entry["sha256"]}`)\n\n')
        for name, path in sorted(files.items()):
            text = path.read_text(encoding='utf-8')
            out.extend([f'### {name}\n\n~~~~text\n',text,'\n' if not text.endswith('\n') else '', '~~~~\n'])
    return ''.join(out), len(packages)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument('--output', type=Path)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    supplements = Path(__file__).resolve().parent
    output = args.output or supplements / 'RUST-DEPENDENCIES.md'
    content, count = generate(args.project.resolve(), supplements)
    if args.check:
        if not output.is_file() or output.read_text() != content:
            raise SystemExit('Rust dependency notices are missing or stale')
    else:
        output.write_text(content)
    print(f'{count} registry packages; {output}')


if __name__ == '__main__': main()
