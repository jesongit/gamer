"""Restore the retained pre-upgrade Gamer container without deleting either volume.

The failed/new container is stopped and renamed for investigation. The original
container must still use the exact image and volume recorded by the checkpoint.
"""
import argparse
import datetime
import json
import pathlib
import subprocess
import time


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--checkpoint',type=pathlib.Path,required=True)
    p.add_argument('--production',required=True)
    p.add_argument('--rollback-container',required=True)
    p.add_argument('--deployment-env',type=pathlib.Path)
    a=p.parse_args()
    receipt=json.loads(a.checkpoint.read_text(encoding='utf-8'))
    def docker(*args):
        return subprocess.check_output(['docker',*args],text=True,encoding='utf-8').strip()
    original=json.loads(docker('inspect',a.rollback_container))[0]
    assert original['Image']==receipt['original_image'],'Rollback image identity differs'
    assert any(x.get('Name')==receipt['original_volume'] and x['Destination']=='/data' for x in original['Mounts'])
    assert original['State']['Running'] is False,'Rollback container is already running'
    current=json.loads(docker('inspect',a.production))[0]
    assert current['Id']!=original['Id']
    assert not any(x.get('Name')==receipt['original_volume'] for x in current['Mounts']), 'Containers must use separate data volumes'
    retained=a.production+'-retained-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%d-%H%M%S')
    existing=docker('ps','-a','--format','{{.Names}}').splitlines()
    assert retained not in existing
    docker('stop','--time','90',a.production)
    docker('rename',a.production,retained)
    try:
        docker('rename',a.rollback_container,a.production)
        docker('start',a.production)
        for _ in range(60):
            state=json.loads(docker('inspect',a.production))[0]['State']
            if state.get('Health',{}).get('Status')=='healthy':break
            if not state['Running']:raise RuntimeError('Rollback service exited; both volumes retained')
            time.sleep(2)
        else:raise TimeoutError('Rollback service did not become healthy')
        if a.deployment_env:
            previous=a.checkpoint.parent/'deployment.env.before'
            assert previous.is_file()
            # Keep the upgraded deployment selection alongside the checkpoint.
            saved=a.checkpoint.parent/('deployment.env.retained-'+retained)
            if a.deployment_env.exists():saved.write_bytes(a.deployment_env.read_bytes())
            a.deployment_env.write_bytes(previous.read_bytes())
        print(json.dumps({'rolled_back':True,'production':a.production,'retained_container':retained,
                          'image':original['Image'],'volume':receipt['original_volume'],'health':'healthy',
                          'removed_data':False}))
    except BaseException:
        print('Rollback did not complete. Both containers and volumes are retained; inspect before retrying.')
        raise


if __name__=='__main__':main()
