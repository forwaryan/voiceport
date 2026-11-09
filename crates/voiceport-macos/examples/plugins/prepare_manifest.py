#!/usr/bin/python3
"""Write a local manifest with the actual absolute adapter path; never run/install it."""
import argparse
import json
import pathlib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', required=True, type=pathlib.Path)
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parent
manifest = json.loads((root / 'reference-manifest.json').read_text())
manifest['program']['args'][1] = str(root / 'reference_adapter.py')
# Exclusive creation preserves existing files.
with args.output.open('x') as output:
    json.dump(manifest, output, ensure_ascii=False, indent=2)
    output.write('\n')
print(args.output.resolve())
