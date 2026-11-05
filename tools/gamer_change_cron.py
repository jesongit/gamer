"""Change a task cron through Gamer's existing MCP snapshot CAS.

Defaults to a read-only preview in the server timezone. --apply saves the task
without running it. Requires task.read/task.write and, for enabled tasks,
run.submit. Gamer's normal restricted external execution policy still applies.
Never edit schedule_json directly: its persisted next_wakeup must be invalidated
by task_save. No SQLite access and no automatic retry after an uncertain write.
"""
import argparse
import copy
import json
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid


class McpClient:
    def __init__(self, url, token):
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme not in ('http', 'https') or not parsed.hostname or parsed.username or parsed.password:
            raise ValueError('Use an HTTP(S) MCP URL without embedded credentials')
        self.url = url.rstrip('/')
        self.token = token
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def call(self, method, params):
        body = {'jsonrpc': '2.0', 'id': uuid.uuid4().hex, 'method': method, 'params': params}
        request = urllib.request.Request(self.url, json.dumps(body).encode(), method='POST', headers={
            'Authorization': 'Bearer ' + self.token,
            'Content-Type': 'application/json',
            'Accept': 'application/json, text/event-stream',
        })
        try:
            with self.opener.open(request, timeout=30) as response:
                text = response.read(2 * 1024 * 1024).decode('utf-8')
                if response.headers.get_content_type() == 'text/event-stream':
                    messages = [json.loads(line[5:].strip()) for line in text.splitlines() if line.startswith('data:')]
                    result = next(item for item in messages if item.get('id') == body['id'])
                else:
                    result = json.loads(text)
        except urllib.error.HTTPError as error:
            raise RuntimeError(f'MCP HTTP {error.code}; no automatic retry') from error
        if 'error' in result:
            raise RuntimeError('MCP rejected request: ' + str(result['error'].get('message', 'unknown error')))
        return result['result']

    def tool(self, name, args):
        result = self.call('tools/call', {'name': name, 'arguments': args})
        if result.get('isError'):
            raise RuntimeError('MCP rejected ' + name + ': ' + str(result.get('structuredContent', result.get('content'))))
        if 'structuredContent' in result:
            return result['structuredContent']
        return json.loads(next(item['text'] for item in result['content'] if item.get('type') == 'text'))


def change_cron(client, task_id, expected_expression, expression, apply=False, operation_id=None):
    task = client.tool('gamer.core.task_get', {'task_id': task_id})
    if task['schedule']['provider_id'] != 'cron':
        raise ValueError('The selected task is not a cron task')
    if task['schedule']['config'].get('expression') != expected_expression:
        raise ValueError('The task cron changed; re-read it before changing it')
    if task.get('enabled') and task.get('state') != 'active':
        raise ValueError('Do not implicitly resume a suspended task while changing its cron')
    schedule = copy.deepcopy(task['schedule'])
    schedule['config']['expression'] = expression
    preview = client.tool('gamer.core.schedule_preview', {'schedule': schedule, 'count': 3})
    if not preview.get('valid') or not preview.get('next_times'):
        raise ValueError('The server did not accept a future cron occurrence')
    result = {'task_id': task_id, 'name': task['name'], 'old_expression': expected_expression,
              'expression': expression, 'server_now_local': preview['server_now_local'],
              'next_times_local': preview['next_times_local'], 'applied': False}
    if not apply:
        return result
    # Explicit fields also make older hosts reject safely instead of silently
    # losing multi-input configuration when rebuilding the task.
    args = {'task_id': task_id, 'expected_updated_at': task['updated_at'],
            'name': task['name'], 'device_id': task['app']['device_id'],
            'package_id': task['app']['content_package'], 'runner_id': task['runner_id'],
            'entrypoint': task['entrypoint'], 'payload': task['payload'],
            'schedule': schedule, 'enabled': task['enabled'],
            'inputs': task.get('inputs', []), 'concurrency': max(1, task.get('concurrency', 1)),
            'operation_id': operation_id or uuid.uuid4().hex}
    print('Saving with operation_id=' + args['operation_id'] + '; do not automatically retry an uncertain result', file=sys.stderr, flush=True)
    saved = client.tool('gamer.core.task_save', args)
    for field in ('schedule', 'inputs', 'enabled', 'concurrency'):
        if saved.get(field) != args[field]:
            raise RuntimeError('Saved task differs in ' + field + '; inspect it before any retry')
    result.update(applied=True, operation_id=args['operation_id'], next_wakeup=saved.get('next_wakeup'))
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True, help='Gamer /api/mcp URL')
    parser.add_argument('--token-file', required=True, help='File containing an existing MCP Bearer token')
    parser.add_argument('--task-id', required=True)
    parser.add_argument('--expected-expression', required=True)
    parser.add_argument('--expression', required=True, help='Cron interpreted in the Gamer server timezone')
    parser.add_argument('--operation-id', help='Explicit unique write operation ID; never automatically retried')
    parser.add_argument('--apply', action='store_true', help='Save after server preview; default is read-only')
    args = parser.parse_args(argv)
    token = Path(args.token_file).read_text(encoding='utf-8').strip()
    if not token:
        parser.error('Token file is empty')
    result = change_cron(McpClient(args.url, token), args.task_id, args.expected_expression,
                         args.expression, args.apply, args.operation_id)
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
