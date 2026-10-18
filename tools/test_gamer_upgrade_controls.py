"""Failure boundaries for local promotion/rollback; no Docker daemon is called."""
import contextlib
import io
import hashlib
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import gamer_promote_diagnostics as promote
import gamer_rollback as rollback


class UpgradeControls(unittest.TestCase):
    def test_rollback_refuses_a_different_original_volume_before_stopping_anything(self):
        with tempfile.TemporaryDirectory() as directory:
            checkpoint=pathlib.Path(directory)/'checkpoint.json'
            checkpoint.write_text(json.dumps({'original_image':'old','original_volume':'protected'}))
            original={'Id':'old-id','Image':'old','Mounts':[{'Destination':'/data','Name':'another'}],'State':{'Running':False}}
            args=['rollback','--checkpoint',str(checkpoint),'--production','prod','--rollback-container','previous']
            with mock.patch.object(sys,'argv',args),mock.patch.object(subprocess,'check_output',return_value=json.dumps([original])) as command:
                with self.assertRaises(AssertionError):rollback.main()
                self.assertEqual(command.call_count,1)

    def test_healthy_rollback_retains_the_new_container_and_both_volumes(self):
        with tempfile.TemporaryDirectory() as directory:
            checkpoint=pathlib.Path(directory)/'checkpoint.json'
            checkpoint.write_text(json.dumps({'original_image':'old','original_volume':'old-vol'}))
            containers={
                'previous':{'Id':'old-id','Image':'old','Mounts':[{'Destination':'/data','Name':'old-vol'}],'State':{'Running':False}},
                'prod':{'Id':'new-id','Image':'new','Mounts':[{'Destination':'/data','Name':'new-vol'}],'State':{'Running':True}},
            }
            commands=[]
            def docker(values,**kwargs):
                command=values[1:];commands.append(command)
                if command[0]=='inspect':return json.dumps([containers[command[1]]])
                if command[0]=='ps':return '\n'.join(containers)
                if command[0]=='stop':containers[command[-1]]['State']['Running']=False
                elif command[0]=='rename':containers[command[2]]=containers.pop(command[1])
                elif command[0]=='start':containers[command[1]]['State']={'Running':True,'Health':{'Status':'healthy'}}
                else:raise AssertionError(command)
                return ''
            args=['rollback','--checkpoint',str(checkpoint),'--production','prod','--rollback-container','previous']
            with mock.patch.object(sys,'argv',args),mock.patch.object(subprocess,'check_output',side_effect=docker),contextlib.redirect_stdout(io.StringIO()):rollback.main()
            self.assertEqual(containers['prod']['Id'],'old-id')
            retained=next(value for value in containers.values() if value['Id']=='new-id')
            self.assertFalse(retained['State']['Running'])
            self.assertFalse(any('rm' in command for command in commands))

    def test_failed_docker_run_with_a_leftover_container_restores_the_original(self):
        with tempfile.TemporaryDirectory() as directory:
            root=pathlib.Path(directory);repo=root/'repo';(repo/'tools').mkdir(parents=True)
            (repo/'deploy/docker').mkdir(parents=True);(repo/'deploy/docker/chromium-seccomp.json').write_text('{}')
            folder=root/'checkpoint';folder.mkdir();env=repo/'.env';env.write_text('GAMER_IMAGE=old\nGAMER_DATA_VOLUME=old-vol\n')
            (folder/'deployment.env.before').write_bytes(env.read_bytes())
            archive=folder/'data.tar.gz';archive.write_bytes(b'fixture')
            receipts={
                'checkpoint.json':{'original_container':'prod','original_image':'old','original_volume':'old-vol','candidate':'candidate','restored_volume':'qa-vol','verified_bytes':0,'archive':str(archive),'archive_bytes':7,'archive_sha256':hashlib.sha256(b'fixture').hexdigest()},
                'real-task-acceptance.json':{'all_success':True,'real_account_tasks':[{}]*4},
                'fault-acceptance.json':{'fault_cases':[{},{}]},
                'parallel-acceptance.json':{'all_slots_released':True},
                'ocr-acceptance.json':{'sampled_ocr_error_frame_preserved':True},
                'preservation-acceptance.json':{'changed':[]},
                'download-lifecycle-acceptance.json':{'temporary_files_remaining':0},
                'idle-reclaim-acceptance.json':{'actual_seconds':1801},
                'ui-acceptance.json':{'download_confirmed_by_user':True},
                'resource-observation.json':{'completed':True,'samples':[{'healthy':True,'restarts':0}]},
                'rollback-rehearsal.json':{'original_container_booted':True},
                'file-manifest.json':{'files':{}},
            }
            for name,value in receipts.items():(folder/name).write_text(json.dumps(value))
            containers={
                'prod':{'Id':'old-id','Image':'old','Mounts':[{'Name':'old-vol'}],'State':{'Running':False}},
                'gamer-diagnostics-acceptance-test':{'Id':'qa-id','Image':'new','Mounts':[{'Name':'qa-vol'}],'State':{'Running':True,'Health':{'Status':'healthy'}},'RestartCount':0},
            }
            commands=[]
            def docker(values,**kwargs):
                c=values[1:];commands.append(c)
                if c[0]=='inspect':return json.dumps([containers[c[1]]])
                if c[0]=='ps':return '\n'.join(containers)
                if c[:2]==['image','inspect']:return json.dumps([{'Id':'new'}])
                if c[:2]==['volume','ls']:return 'old-vol\nqa-vol'
                if c[:2]==['volume','create']:return ''
                if c[0]=='stop':containers[c[-1]]['State']['Running']=False;return ''
                if c[0]=='rename':containers[c[2]]=containers.pop(c[1]);return ''
                if c[0]=='tag':return ''
                if c[0]=='exec':return 'idle'
                if c[0]=='start':containers[c[1]]['State']={'Running':True,'Health':{'Status':'healthy'}};return ''
                if c[:2]==['run','-d']:
                    containers['prod']={'Id':'partial-id','Image':'new','Mounts':[{'Name':'new-vol'}],'State':{'Running':False}}
                    raise subprocess.CalledProcessError(125,values)
                if c[0]=='run' and 'readonly' in ' '.join(c):
                    return json.dumps({'files':1,'bytes':1,'all_hashes_equal':True}) if '/source,readonly' in ' '.join(c) else '["existing-run"]'
                if c[0]=='run':return json.dumps({'runs':5})
                raise AssertionError(c)
            args=['promote','--checkpoint',str(folder/'checkpoint.json'),'--acceptance-container','gamer-diagnostics-acceptance-test',
                  '--production-container','prod','--rollback-container','previous','--production-volume','new-vol','--image-tag','tested']
            with mock.patch.object(sys,'argv',args),mock.patch.object(promote,'__file__',str(repo/'tools/gamer_promote_diagnostics.py')),mock.patch.object(subprocess,'check_output',side_effect=docker),contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaises(subprocess.CalledProcessError):promote.main()
            self.assertEqual(containers['prod']['Id'],'old-id')
            self.assertTrue(containers['prod']['State']['Running'])
            self.assertTrue(any(value['Id']=='partial-id' for value in containers.values()))
            self.assertEqual(env.read_bytes(),(folder/'deployment.env.before').read_bytes())
            self.assertFalse(any(command[:2]==['volume','rm'] for command in commands))


if __name__=='__main__':unittest.main()
