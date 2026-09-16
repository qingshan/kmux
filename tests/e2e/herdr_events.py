#!/usr/bin/env python3
"""Fault-injected Herdr socket test using the real proxy and SSH bridge code."""
import collections
import concurrent.futures
import json
import os
from pathlib import Path
import pwd
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]


class Server:
    def __init__(self, path):
        self.listener = socket.socket(socket.AF_UNIX)
        self.listener.bind(path)
        self.listener.listen()
        self.listener.settimeout(.2)
        self.lock = threading.Lock()
        self.stop = threading.Event()
        self.subscribers = []
        self.subscriptions = {}
        self.reject = False
        self.label = 'initial'
        self.text = 'fixture output'
        self.agent = None
        self.agent_state = 'unknown'
        self.counts = collections.Counter()
        self.thread = threading.Thread(target=self.accept, daemon=True)
        self.thread.start()

    def accept(self):
        while not self.stop.is_set():
            try:
                connection, _ = self.listener.accept()
            except socket.timeout:
                continue
            threading.Thread(target=self.handle, args=(connection,), daemon=True).start()

    def handle(self, connection):
        try:
            with connection, connection.makefile('rb') as reader:
                request = json.loads(reader.readline())
                method = request['method']
                with self.lock:
                    self.counts[method] += 1
                    if method == 'events.subscribe':
                        subscriptions = request['params']['subscriptions']
                        assert all(s.get('pane_id') for s in subscriptions if s['type'] == 'pane.agent_status_changed')
                        if self.reject:
                            connection.sendall(b'{"error":{"message":"subscriptions unavailable"}}\n')
                            return
                        connection.sendall(b'{"result":{"type":"subscription_started"}}\n')
                        self.subscribers.append(connection)
                        self.subscriptions[connection] = subscriptions
                        result = None
                    elif method == 'session.snapshot':
                        result = {'snapshot': {
                            'workspaces': [{'workspace_id':'w1','label':'work'}],
                            'tabs': [{'tab_id':'w1:t1','workspace_id':'w1','label':self.label}],
                            'panes': [{'pane_id':'w1:p1','tab_id':'w1:t1','workspace_id':'w1','cwd':'/tmp',
                                       'agent':self.agent,'agent_status':self.agent_state}]}}
                    elif method == 'pane.read':
                        result = {'read':{'text':self.text}}
                    else:
                        result = {'type':'pong' if method == 'ping' else 'ok'}
                if result is not None:
                    connection.sendall((json.dumps({'id':request['id'],'result':result})+'\n').encode())
                else:
                    # Detect the bridge disconnecting; no synthetic heartbeat events.
                    reader.read(1)
        except (OSError, ValueError):
            pass
        finally:
            with self.lock:
                if connection in self.subscribers:
                    self.subscribers.remove(connection)
                self.subscriptions.pop(connection, None)

    def event(self, kind='tab_renamed'):
        with self.lock:
            for connection in list(self.subscribers):
                if kind == 'pane.agent_status_changed' and not any(
                        s['type'] == kind and s.get('pane_id') == 'w1:p1'
                        for s in self.subscriptions.get(connection, [])):
                    continue
                try:
                    connection.sendall((json.dumps({'event':kind,'data':{'tab_id':'w1:t1','pane_id':'w1:p1'}})+'\n').encode())
                except OSError:
                    pass

    def drop(self):
        with self.lock:
            for connection in list(self.subscribers):
                try:
                    connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass

    def count(self, method):
        with self.lock:
            return self.counts[method]


def main():
    with tempfile.TemporaryDirectory(prefix='kmux-events-') as directory:
        server = Server(directory+'/herdr.sock')
        with socket.socket() as listener:
            listener.bind(('127.0.0.1',0))
            port = listener.getsockname()[1]
        cfg = Path(directory)/'proxy.json'
        cfg.write_text(json.dumps({'bind':'127.0.0.1','port':port,'token':'fixture',
            'hosts':[{'id':'fixture','backend':'herdr','target':'local:'+pwd.getpwuid(os.getuid()).pw_name,
                      'herdrSocket':directory+'/herdr.sock'}]}))
        proxy = subprocess.Popen([str(ROOT/'target/debug/kmux-proxy'),str(cfg)],stdout=subprocess.DEVNULL)

        def post(path, **fields):
            body = json.dumps(dict(version=1,token='fixture',machine='fixture',**fields)).encode()
            with urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{port}/v1/'+path,
                body,{'Content-Type':'application/json'}),timeout=12) as response:
                return json.load(response)

        def action(op='snapshot', **fields):
            return post('action',action=dict(op=op,**fields))

        def until(predicate, timeout=8):
            end = time.monotonic()+timeout
            while time.monotonic()<end:
                snapshot=action()
                if predicate(snapshot):
                    return snapshot
                time.sleep(.05)
            raise AssertionError('event condition timed out')

        try:
            time.sleep(.2)
            snapshot=until(lambda s:s['terminal']['eventUpdates'])
            # Bootstrap causes a second subscription containing explicit pane IDs.
            until(lambda s:server.count('events.subscribe') >= 2)
            # Quiet terminal refreshes read output, but do not re-fetch inventory.
            count=server.count('session.snapshot')
            for _ in range(8):
                snapshot=action()
            assert server.count('session.snapshot')==count
            assert server.count('pane.read')>=9
            with concurrent.futures.ThreadPoolExecutor() as pool:
                after=snapshot['terminal']['eventRevision']
                future=pool.submit(post,'changes',after=after,timeout_ms=3000)
                time.sleep(.1)
                assert not future.done(), 'idle waiter did not wait'
                server.label='renamed externally'
                server.event()
                assert future.result(timeout=1)['revision']!=after
                snapshot=until(lambda s:s['tabs'][0]['name']==server.label)
                # An idle long-poll must never block input or consume an RPC reply.
                future=pool.submit(post,'changes',after=snapshot['terminal']['eventRevision'],timeout_ms=3000)
                time.sleep(.1)
                start=time.monotonic()
                action('text',text='exactly once')
                assert time.monotonic()-start<1
                future.result(timeout=1)
                assert server.count('pane.send_text')==1
            # Output without any lifecycle event must still appear on the next capture.
            server.text='delayed output without metadata event'
            assert server.text in '\n'.join(action()['terminal']['screen'])
            server.agent = 'reviewer'
            for state in ['blocked', 'working', 'done', 'idle', 'unknown']:
                server.agent_state = state
                server.event('pane.agent_status_changed')
                snap = until(lambda s:s['panes'][0].get('agent',{}).get('state') == state)
                assert snap['sessions'][0]['agents'] == {'state':state,'count':1}
                assert snap['tabs'][0]['agents'] == {'state':state,'count':1}
                assert snap['panes'][0]['agent']['source'] == 'herdr'
            server.agent = None
            server.event('pane.agent_detected')
            snap = until(lambda s:'agent' not in s['panes'][0])
            assert 'agents' not in snap['sessions'][0] and 'agents' not in snap['tabs'][0]
            action('copy_enter')
            server.event()
            assert action()['terminal']['copyMode']
            action('copy_key',key='q')
            # Dropping only the event socket leaves ordinary RPC and input usable.
            server.reject=True
            server.drop()
            until(lambda s:not s['terminal']['eventUpdates'])
            count=server.count('session.snapshot')
            server.label='fallback rename'
            assert action()['tabs'][0]['name']==server.label
            assert server.count('session.snapshot')>count
            action('text',text='fallback once')
            assert server.count('pane.send_text')==2
            server.reject=False
            until(lambda s:s['terminal']['eventUpdates'])
            # Lost events cannot leave the cache stale forever.
            server.label='resynchronized'
            snapshot=until(lambda s:s['tabs'][0]['name']==server.label,timeout=12)
            action('detach')
            end=time.monotonic()+3
            while server.subscribers and time.monotonic()<end:
                time.sleep(.05)
            assert not server.subscribers, 'subscription leaked after detach'
            print('Herdr events: cache, wakeup, concurrent input, output fallback, reconnect, resync, detach OK')
        finally:
            proxy.terminate()
            proxy.wait(timeout=5)
            server.stop.set()
            server.drop()
            server.thread.join(timeout=1)
            server.listener.close()


if __name__=='__main__':
    main()
