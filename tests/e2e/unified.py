#!/usr/bin/env python3
"""Exercise the same public contract against isolated real tmux and Herdr servers."""
import json
import os
from pathlib import Path
import pwd
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.request
import urllib.error

ROOT = Path(__file__).resolve().parents[2]

def main():
    name = 'kmux-e2e-' + str(os.getpid())
    user = pwd.getpwuid(os.getuid()).pw_name
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix='kmux-unified-') as directory:
        cfg = Path(directory) / 'proxy.json'
        cfg.write_text(json.dumps({'bind':'127.0.0.1','port':port,'token':'fixture',
            'defaultHost':'tmux','hosts':[
                {'id':'tmux','target':'local:'+user,'session':'main','tmuxSocket':name},
                {'id':'herdr','target':'local:'+user,'backend':'herdr','herdrSession':name},
                {'id':'herdr-autostart','target':'local:'+user,'backend':'herdr','herdrSession':name+'-auto'},
                {'id':'empty','target':'local:'+user,'session':'must-not-create','tmuxSocket':name+'-empty'},
                {'id':'offline','target':'local:'+user,'backend':'herdr','herdrSocket':'/tmp/'+name+'-missing.sock'}]}))
        herdr = subprocess.Popen(['herdr','--session',name,'server'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        proxy = subprocess.Popen([str(ROOT/'target/debug/kmux-proxy'),str(cfg)], stdout=subprocess.DEVNULL)
        def call(machine, action, expected=None):
            body=json.dumps({'version':1,'token':'fixture','machine':machine,'action':action,'expected_pane':expected}).encode()
            request=urllib.request.Request(f'http://127.0.0.1:{port}/v1/action',body,{'Content-Type':'application/json'})
            try:
                with urllib.request.urlopen(request,timeout=25) as response:
                    return json.load(response)
            except urllib.error.HTTPError as error:
                raise RuntimeError(error.read().decode()) from error
        def wait(machine,predicate, timeout=15):
            until=time.monotonic()+timeout
            while time.monotonic()<until:
                snap=call(machine,{'op':'snapshot'})
                if predicate(snap): return snap
                time.sleep(.2)
            raise AssertionError('snapshot condition timed out: '+machine+' panes='+repr(snap['panes']))
        try:
            time.sleep(1)
            for machine in ['tmux','herdr']:
                snap=call(machine,{'op':'snapshot'})
                if not snap['sessions']:
                    snap=call(machine,{'op':'create_session','name':'test space'})
                assert snap['sessions'] and snap['tabs'] and snap['panes'], snap
                assert snap['terminal']['eventUpdates'] == (machine == 'herdr')
                if machine == 'tmux':
                    def tmux_wh(target):
                        return subprocess.check_output(['tmux','-L',name,'display-message','-p','-t',
                            target,'#{window_width} #{window_height}'],text=True).strip()
                    subprocess.run(['tmux','-L',name,'resize-window','-t',snap['selection']['tab'],
                                    '-x','120','-y','40'],check=True,capture_output=True)
                    call(machine,{'op':'snapshot'})
                    assert tmux_wh(snap['selection']['pane'])=='80 24'
                    first=snap['selection'].copy()
                    other=call(machine,{'op':'create_session','name':'kmux-fit'})
                    other_sel=other['selection'].copy()
                    call(machine,{'op':'select','session':first['session']})
                    subprocess.run(['tmux','-L',name,'resize-window','-t',other_sel['tab'],
                                    '-x','120','-y','40'],check=True,capture_output=True)
                    assert tmux_wh(other_sel['pane'])=='120 40'
                    call(machine,{'op':'select','session':other_sel['session']})
                    assert tmux_wh(other_sel['pane'])=='80 24', 'session switch must resize the new window'
                    call(machine,{'op':'select','session':first['session'],'tab':first['tab'],
                                  'pane':first['pane']})
                pane=snap['selection']['pane']
                call(machine,{'op':'text','text':'exec sh'},pane)
                call(machine,{'op':'key','key':'Enter'},pane)
                time.sleep(.3)
                text="printf 'KMUX_UNIFIED_λ\\n'"
                call(machine,{'op':'text','text':text},pane)
                call(machine,{'op':'key','key':'Enter'},pane)
                snap=wait(machine,lambda s:'KMUX_UNIFIED_λ' in '\n'.join(s['terminal']['screen']))
                assert len(snap['terminal']['screen'])==24
                assert not snap['terminal']['copyMode']
                try:
                    call(machine,{'op':'copy_key','key':'Enter'},pane)
                    raise AssertionError('copy key outside copy mode accepted')
                except RuntimeError as e:
                    assert 'enter copy mode first' in str(e)
                literal = "$KMUX_VARIABLE_THAT_IS_NOT_SET; quotes ' \" \\ λ"
                # The remote shell receives a single-quoted printf argument;
                # tmux must not expand the dollar expression on the way in.
                shell_literal = "'" + literal.replace("'", "'\\''") + "'"
                call(machine,{'op':'text','text':"printf '%s\\n' "+shell_literal},pane)
                call(machine,{'op':'key','key':'Enter'},pane)
                wait(machine,lambda s:literal in '\n'.join(s['terminal']['screen']))
                snap=call(machine,{'op':'copy_enter'},pane)
                assert snap['terminal']['copyMode']
                call(machine,{'op':'copy_search','query':'KMUX_UNIFIED_λ','backwards':True},pane)
                call(machine,{'op':'copy_key','key':'V'},pane)
                snap=call(machine,{'op':'copy_key','key':'y'},pane)
                assert 'KMUX_UNIFIED_λ' in snap['terminal']['clipboard']
                assert not snap['terminal']['copyMode']
                old=snap['selection'].copy()
                snap=call(machine,{'op':'create_tab','cwd':'/tmp'})
                assert snap['selection']['tab']!=old['tab']
                new=snap['selection'].copy()
                snap=call(machine,{'op':'next_tab','previous':True})
                assert snap['selection']['tab']==old['tab']
                snap=call(machine,{'op':'next_tab','previous':False})
                assert snap['selection']['tab']==new['tab']
                try:
                    call(machine,{'op':'text','text':'MUST_NOT_SEND'},old['pane'])
                    raise AssertionError('stale input accepted')
                except RuntimeError as e:
                    assert 'pane changed' in str(e)
                call(machine,{'op':'close_tab'})
                snap=call(machine,{'op':'select',**old})
                snap=call(machine,{'op':'list_files','path':'/tmp'})
                assert snap['terminal']['cwd']=='/tmp'
                snap=call(machine,{'op':'create_session','name':'second-workspace'})
                assert snap['selection']['session']!=old['session']
                assert any(s['name']=='second-workspace' for s in snap['sessions'])
                call(machine,{'op':'select',**old})
                detached=call(machine,{'op':'detach'})
                assert not detached['terminal']['connected']
                call(machine,{'op':'snapshot'})
                print(machine+': inventory, input, Unicode, snapshot, copy/search/yank, tabs, stale input, files OK',flush=True)
            snap=call('tmux',{'op':'snapshot'})
            original=snap['selection'].copy()
            call('tmux',{'op':'copy_enter'})
            def catalog_ready(s):
                c={g['machine']:g for g in s['terminal']['sessionCatalog']}
                return (len(c)==5 and all(not g['loading'] for g in c.values())
                        and c['herdr']['sessions'] and not c['herdr-autostart']['unavailable']
                        and c['offline']['unavailable'])
            snap=wait('tmux',catalog_ready,timeout=35)
            assert snap['selection']==original and snap['terminal']['copyMode']
            report=call('tmux',{'op':'diagnose'})['terminal']['diagnose']
            assert report['proxy']['ok'] and not report.get('running')
            by_id={h['id']:h for h in report['hosts']}
            assert set(by_id)=={'tmux','herdr','herdr-autostart','empty','offline'}
            assert by_id['tmux']['ok'] and by_id['tmux']['ssh']['ok'] and 'session' in by_id['tmux']['service']['detail']
            assert by_id['herdr']['ok'] and 'workspace' in by_id['herdr']['service']['detail']
            assert by_id['empty']['ssh']['ok'] and not by_id['offline']['ok']
            snap=call('tmux',{'op':'snapshot'})
            assert snap['selection']==original and snap['terminal']['copyMode'], 'diagnose must not leave copy mode'
            catalog={g['machine']:g for g in snap['terminal']['sessionCatalog']}
            assert catalog['tmux']['sessions'] and catalog['herdr']['sessions']
            assert catalog['tmux']['panes'] and catalog['herdr']['panes']
            assert all('session' in p and 'tab' in p and 'id' in p
                       for machine in ('tmux','herdr') for p in catalog[machine]['panes'])
            assert catalog['tmux']['tabs'] and catalog['herdr']['tabs']
            assert not catalog['herdr-autostart']['unavailable'], 'proxy should start a stopped Herdr server'
            sessions = subprocess.check_output(['herdr','session','list'], text=True).lower()
            assert name+'-auto' in sessions and 'running' in sessions, sessions
            assert not catalog['empty']['sessions'] and not catalog['empty']['unavailable']
            assert not catalog['empty']['panes'] and not catalog['offline']['panes']
            assert subprocess.run(['tmux','-L',name+'-empty','list-sessions'],capture_output=True).returncode != 0
            target=catalog['herdr']['sessions'][-1]['id']
            snap=call('herdr',{'op':'select','session':target})
            assert snap['selection']['session']==target and snap['machine']=='herdr'
            # Change Herdr outside kmux and verify that its real subscription
            # wakes a versioned waiter and invalidates the cached inventory.
            revision=snap['terminal']['eventRevision']
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(5)
                connection.connect(str(Path.home()/'.config/herdr/sessions'/name/'herdr.sock'))
                connection.sendall((json.dumps({'id':'external-rename','method':'workspace.rename',
                    'params':{'workspace_id':target,'label':'event-renamed'}})+'\n').encode())
                reply=json.loads(connection.makefile('rb').readline())
                assert 'error' not in reply, reply
            body=json.dumps({'version':1,'token':'fixture','machine':'herdr','after':revision,'timeout_ms':3000}).encode()
            with urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{port}/v1/changes',body,
                {'Content-Type':'application/json'}),timeout=5) as response:
                change=json.load(response)
            assert change['event_updates'] and change['revision']!=revision
            wait('herdr',lambda s:any(v['name']=='event-renamed' for v in s['sessions']))
            print('real Herdr subscription: external rename, change wakeup, inventory invalidation OK',flush=True)
            # Report fixture state only on our isolated Herdr server. No real
            # agent is launched, prompted, or approved by this test.
            snap=call('herdr',{'op':'snapshot'})
            agent_pane=snap['selection']['pane']
            for state in ['working', 'blocked', 'unknown']:
                subprocess.run(['herdr','--session',name,'pane','report-agent',agent_pane,
                    '--source','custom:kmux-e2e','--agent','kmux-fixture','--state',state],
                    check=True,stdout=subprocess.DEVNULL)
                snap=wait('herdr',lambda s:any(p.get('agent',{}).get('state')==state
                    and p['id']==agent_pane for p in s['panes']),timeout=8)
                assert next(s for s in snap['sessions'] if s['id']==target)['agents']['state']==state
                assert next(t for t in snap['tabs'] if t['id']==snap['selection']['tab'])['agents']['count']==1
            chosen=snap['selection'].copy()
            call('herdr',{'op':'create_tab','cwd':'/tmp'})
            snap=call('herdr',{'op':'select',**chosen})
            assert snap['selection']==chosen, 'agent selection must target its exact pane'
            print('real Herdr agent awareness: reported states, rollups and exact pane selection OK',flush=True)
            # A harmless executable-name fixture tests tmux detection without
            # starting a coding agent or consuming any external service.
            fixture=Path(directory)/'codex'
            shutil.copy(shutil.which('sleep'),fixture)
            subprocess.run(['tmux','-L',name,'new-window','-d','-t','main',str(fixture),'120'],check=True)
            call('herdr',{'op':'copy_enter'})
            def group(snapshot,machine):
                return next(g for g in snapshot['terminal']['sessionCatalog'] if g['machine']==machine)
            # Inactive tmux discovery must find agents without an attach or
            # interrupting the Herdr pane/copy selection.
            snap=wait('herdr',lambda s:bool(group(s,'tmux')['agent_panes']),timeout=25)
            assert snap['selection']==chosen and snap['terminal']['copyMode']
            assert not subprocess.check_output(['tmux','-L',name,'list-clients'],text=True).strip()
            detected=group(snap,'tmux')['agent_panes'][0]
            assert group(snap,'tmux')['tabs']
            assert not group(snap,'empty')['agent_panes'] and not group(snap,'offline')['agent_panes']
            assert detected['agent']=={'name':'codex','state':'unknown','source':'command'}
            snap=call('tmux',{'op':'select','session':detected['session'],'tab':detected['tab'],'pane':detected['id']})
            assert snap['selection']['pane']==detected['id']
            # Likewise, background Herdr discovery supplies the other host's
            # exact IDs and names while keeping tmux selected and in copy mode.
            tmux_selection=snap['selection'].copy()
            call('tmux',{'op':'copy_enter'})
            snap=wait('tmux',lambda s:any(p['id']==agent_pane for p in group(s,'herdr')['agent_panes']),timeout=25)
            assert snap['selection']==tmux_selection and snap['terminal']['copyMode']
            assert group(snap,'herdr')['tabs']
            call('tmux',{'op':'copy_key','key':'q'})
            snap=call('tmux',{'op':'close_tab'})
            assert not any(p.get('agent') for p in snap['panes'])
            try:
                call('tmux',{'op':'select','session':detected['session'],'tab':detected['tab'],'pane':detected['id']})
                raise AssertionError('closed agent target was accepted')
            except RuntimeError as error:
                assert 'no longer exists' in str(error)
            print('real tmux agent awareness: command identity, unknown state, selection and cleanup OK',flush=True)
            print('cross-host agents: read-only discovery, exact targets, names, offline/empty hosts and copy preservation OK',flush=True)
            print('cross-host catalog: sessions, direct selection, offline/empty hosts, active copy preservation OK',flush=True)
            print('unified API E2E: ok',flush=True)
        finally:
            proxy.terminate()
            proxy.wait(timeout=10)
            subprocess.run(['tmux','-L',name,'kill-server'],check=False,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            subprocess.run(['herdr','session','stop',name],check=False,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            subprocess.run(['herdr','session','stop',name+'-auto'],check=False,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            herdr.terminate()
            herdr.wait(timeout=10)

if __name__=='__main__': main()
