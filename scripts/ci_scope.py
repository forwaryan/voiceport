#!/usr/bin/env python3
"""Select the CI scope from a complete local Git diff, defaulting to full checks."""

import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
DOC_SUFFIXES = {'.md', '.png', '.webp', '.svg', '.jpg', '.jpeg', '.gif'}
SHA = re.compile(r'[0-9a-f]{40}')


def documentation_path(path):
    """Only README and gallery assets qualify; other docs may be build inputs.

    In particular, docs/TRANSCRIPTION_PLUGINS.md is embedded into the CLI.
    """
    name = PurePosixPath(path)
    return path == 'README.md' or (
        path.startswith('docs/images/') and name.suffix in DOC_SUFFIXES
    )


def git(repo, *args):
    return subprocess.run(
        ['git', *args], cwd=repo, check=True, capture_output=True, timeout=60,
    ).stdout


def scope(event_name, event, repo=ROOT):
    # Always fully validate main, including documentation merges. Only PRs use
    # the fast path, and the existing required job still runs and reports scope.
    if event_name != 'pull_request':
        return True, '', 'non-PR event'
    try:
        base = event['pull_request']['base']['sha']
        if not isinstance(base, str) or not SHA.fullmatch(base):
            return True, '', 'invalid base commit'
        try:
            git(repo, 'cat-file', '-e', base + '^{commit}')
        except subprocess.CalledProcessError:
            # checkout is intentionally shallow. Fetch only the exact base,
            # rather than downloading every branch and the complete history.
            git(repo, 'fetch', '--no-tags', '--depth=1', 'origin', base)
        # HEAD is the actual checkout (normally GitHub's PR merge commit).
        # Disable rename detection so moving source into docs cannot hide the
        # deleted source path. NUL delimiters preserve unusual file names.
        changed = git(repo, 'diff', '--name-only', '--no-renames', '-z', base, 'HEAD', '--')
        paths = [os.fsdecode(path) for path in changed.split(b'\0') if path]
        if not paths or not all(documentation_path(path) for path in paths):
            return True, base, 'code, configuration, unknown input, or empty diff'
        return False, base, 'documentation-only PR'
    except (KeyError, TypeError, OSError, subprocess.SubprocessError) as error:
        print(f'CI scope unavailable; running all checks: {error}', file=sys.stderr)
        return True, '', 'scope could not be established'


def main():
    try:
        with open(os.environ['GITHUB_EVENT_PATH'], encoding='utf-8') as source:
            event = json.load(source)
        full, base, reason = scope(os.environ.get('GITHUB_EVENT_NAME', ''), event)
    except (KeyError, OSError, ValueError, TypeError) as error:
        print(f'CI event unavailable; running all checks: {error}', file=sys.stderr)
        full, base, reason = True, '', 'event could not be established'
    print(f'full={str(full).lower()}')
    print(f'base={base}')
    print(f'reason={reason}')


if __name__ == '__main__':
    main()
