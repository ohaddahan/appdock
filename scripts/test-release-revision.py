#!/usr/bin/env python3
"""Release provenance regressions; GitHub writes are replaced by a local recorder."""
import os
from pathlib import Path
import subprocess
import tempfile

script = Path(__file__).with_name('release-revision.sh').resolve()
with tempfile.TemporaryDirectory(prefix='appdock-release-tests-') as temporary:
    root = Path(temporary)
    binaries, repo = root / 'bin', root / 'repo'
    binaries.mkdir()
    repo.mkdir()
    recorder = binaries / 'gh'
    recorder.write_text('#!/bin/sh\nprintf "%s\\n" "$@" >> "$GH_CALLS"\n')
    recorder.chmod(0o755)

    def git(*args):
        return subprocess.check_output(['git', *args], cwd=repo, text=True, stderr=subprocess.DEVNULL).strip()

    git('init')
    for key, value in [('user.name', 'Fixture'), ('user.email', 'fixture@example.invalid'),
                       ('commit.gpgsign', 'false'), ('tag.gpgsign', 'false'), ('core.hooksPath', '/dev/null')]:
        git('config', key, value)
    manifest = repo / 'Cargo.toml'
    manifest.write_text('[package]\nname="appdock"\nversion="0.1.0"\n')
    git('add', 'Cargo.toml')
    git('commit', '-m', 'fixture')
    sha = git('rev-parse', 'HEAD')
    env = dict(os.environ, PATH=f'{binaries}:' + os.environ['PATH'],
               GH_CALLS=str(root / 'gh-calls'), GITHUB_OUTPUT=str(root / 'outputs'),
               GITHUB_REPOSITORY='fixture/appdock')

    def run(event, tag='master', kind='branch'):
        (root / 'outputs').write_text('')
        (root / 'gh-calls').write_text('')
        result = subprocess.run(['bash', str(script)], cwd=repo, capture_output=True, text=True,
                                env=dict(env, EVENT_NAME=event, EVENT_TAG=tag, REF_TYPE=kind))
        return result, (root / 'outputs').read_text(), (root / 'gh-calls').read_text()

    result, output, calls = run('workflow_dispatch')
    assert result.returncode == 0, result.stderr
    assert 'tag=v0.1.0' in output and sha in output and 'refs/tags/v0.1.0' in calls
    for event in ['push', 'release', 'pull_request']:
        result, _, calls = run(event)
        assert result.returncode != 0 and not calls
    git('tag', '-a', 'v0.1.0', '-m', 'fixture release')
    for event in ['workflow_dispatch']:
        result, output, calls = run(event, 'v0.1.0', 'tag')
        assert result.returncode == 0, (event, result.stderr)
        assert 'tag=v0.1.0' in output and not calls
    (repo / 'change').write_text('new commit')
    git('add', 'change')
    git('commit', '-m', 'later')
    result, _, calls = run('workflow_dispatch')
    assert result.returncode != 0 and 'another commit' in result.stdout and not calls
    manifest.write_text('[package]\nname="appdock"\nversion="0.2.0-rc.1"\n')
    git('add', 'Cargo.toml')
    git('commit', '-m', 'version bump')
    result, output, calls = run('workflow_dispatch')
    assert result.returncode == 0 and 'tag=v0.2.0-rc.1' in output and 'refs/tags/v0.2.0-rc.1' in calls
    manifest.write_text('[package]\nname="appdock"\nversion="invalid"\n')
    result, _, calls = run('workflow_dispatch')
    assert result.returncode != 0 and not calls
    print('8 release revision scenarios passed; no remote changes.')
