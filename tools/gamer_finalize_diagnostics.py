"""Resume only the receipt/config finalization of this verified local cutover.

No container is created, started, or removed. Both retained volumes must match
the checkpoint, and original file/row preservation must already have passed.
"""
import argparse
import datetime
import json
import pathlib
import shutil
import subprocess

from gamer_promote_diagnostics import VERIFY


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkpoint', type=pathlib.Path, required=True)
    args = parser.parse_args()
    folder = args.checkpoint.resolve().parent
    checkpoint = json.loads(args.checkpoint.read_text(encoding='utf-8'))

    def docker(*values):
        return subprocess.check_output(['docker', *values], text=True, encoding='utf-8').strip()

    def inspect(name):
        return json.loads(docker('inspect', name))[0]

    production_name = 'gamer-production-gamer-1'
    rollback_name = 'gamer-production-rollback-20261018'
    acceptance_name = 'gamer-diagnostics-acceptance-20261018'
    production_volume = 'gamer-production-data-diag-20261018'
    tag = 'gamer:diagnostics-20261018'
    production, rollback, acceptance = map(inspect, [production_name, rollback_name, acceptance_name])
    image = json.loads(docker('image', 'inspect', checkpoint['candidate']))[0]['Id']
    assert checkpoint['original_container'] == production_name
    assert production['Image'] == image == acceptance['Image']
    assert production['Config']['Labels']['gamer.release'] == 'diagnostics-20261018'
    assert production['State']['Health']['Status'] == 'healthy'
    assert production['RestartCount'] == 0
    assert any(x.get('Name') == production_volume and x['Destination'] == '/data' for x in production['Mounts'])
    assert rollback['Image'] == checkpoint['original_image'] and not rollback['State']['Running']
    assert any(x.get('Name') == checkpoint['original_volume'] for x in rollback['Mounts'])
    assert not acceptance['State']['Running']
    assert any(x.get('Name') == checkpoint['restored_volume'] for x in acceptance['Mounts'])
    assert json.loads(docker('image', 'inspect', tag))[0]['Id'] == image
    preservation = json.loads((folder / 'production-preservation.json').read_text())
    assert not preservation['changed'] and preservation['original_rows_unchanged']['run_records'] == 164
    verified = json.loads(docker('exec', production_name, 'python3', '-c', VERIFY))
    assert verified['runs'] == 168
    # Confirm fixture records are still in the acceptance volume and absent
    # from production; never alter either database here.
    rows = """import sqlite3,json
db=sqlite3.connect('file:/data/gamer.db?mode=ro',uri=True)
print(json.dumps([key for key,text in db.execute('select run_id,record from run_records') if json.loads(text).get('content_package')=='diagnostics-acceptance-20261018']))
"""
    synthetic = json.loads(docker('run', '--rm', '--memory', '512m', '--memory-swap', '512m', '--entrypoint', 'python3', '--mount',
        'type=volume,src=' + checkpoint['restored_volume'] + ',dst=/data,readonly', checkpoint['candidate'], '-c', rows))
    assert len(synthetic) == 9
    assert json.loads(docker('exec', production_name, 'python3', '-c', rows)) == []
    repo = pathlib.Path(__file__).resolve().parents[1]
    selection = repo / '.env'
    previous = folder / 'deployment.env.before'
    assert selection.read_bytes() == previous.read_bytes(), 'Selection changed; inspect before resuming'
    assert not (folder / 'promotion.json').exists(), 'Already finalized'
    shutil.copy2(repo / 'tools/gamer_rollback.py', folder / 'rollback.py')
    result = {
        'production_container': production_name, 'production_volume': production_volume,
        'image_tag': tag, 'image_id': image, 'rollback_container': rollback_name,
        'rollback_volume': checkpoint['original_volume'],
        'acceptance_volume_kept': checkpoint['restored_volume'],
        'finalized_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'finalization_resumed_after_controller_interruption': True,
        'copy_gate': 'Cold copy hash gate preceded creation; independent production preservation receipt passed',
        'preparation': {'synthetic_runs_removed_from_new_production_only': synthetic,
                        'acceptance_source_kept': True, 'runs': 168, 'enabled_tasks': 4},
        'verification': verified, 'pushed_to_hub': False,
    }
    (folder / 'promotion.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    updated = []
    for line in previous.read_text(encoding='utf-8').splitlines():
        if line.startswith('GAMER_IMAGE='):
            line = 'GAMER_IMAGE=' + tag
        elif line.startswith('GAMER_DATA_VOLUME='):
            line = 'GAMER_DATA_VOLUME=' + production_volume
        updated.append(line)
    temporary = selection.with_suffix('.env.tmp')
    temporary.write_text('\n'.join(updated) + '\n', encoding='utf-8')
    temporary.replace(selection)
    print(json.dumps(result))


if __name__ == '__main__':
    main()
