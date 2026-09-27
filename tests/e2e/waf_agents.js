/* Agent awareness uses real WAF rendering and routing, without remote input. */
var assert = require('assert');
var fs = require('fs');
var vm = require('vm');
var elements = { 'window-tabs': { innerHTML: '' }, 'menu-list': { innerHTML: '' },
    'f-menu-search': { value: '' } };
var sent = [];
var context = {
    document: { addEventListener: function () {} },
    setTimeout: function () {}, clearTimeout: function () {}, Date: Date,
    byId: function (id) { return elements[id] || null; },
    esc: function (s) { return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;'); },
    sendJsonCmd: function (service, op) { sent.push(op); return true; }
};
vm.createContext(context);
vm.runInContext(fs.readFileSync('kpm/waf/script.js', 'utf8'), context);
context.beginPaneSwitch = function () {};
context.closeWinMenu = function () {};
context.showView = function (view) { assert.equal(view, 'view'); };
['tmux', 'herdr'].forEach(function (backend) {
    var session = backend === 'tmux' ? '$1' : 'w1';
    var tab = backend === 'tmux' ? '@2' : 'w1:t2';
    var pane = backend === 'tmux' ? '%3' : 'w1:p3';
    var summary = { state: backend === 'tmux' ? 'unknown' : 'blocked', count: 1 };
    var data = { activeHost: backend, session: session,
        windows: ['W ' + session + ' ' + tab + ' Shell *'], panes: [],
        sessionNames: {}, tabNames: {}, tabAgents: {},
        hosts: [{id: backend, name: backend}],
        sessionCatalog: [{machine: backend, sessions: [{id: session, name: 'Project', agents: summary}]}],
        agentPanes: [{id: pane, tab: tab, session: session,
            agent: {name: 'Review <agent>', state: summary.state}}]
    };
    data.tabAgents[tab] = summary;
    context.lastData = data;
    context.menuGroup = 'sessions';
    context.renderTabs(data);
    assert(elements['window-tabs'].innerHTML.includes(backend === 'tmux' ? '[Unknown]' : '[Needs input]'));
    elements['f-menu-search'].value = '';
    context.renderWindowMenu(data);
    assert(!elements['menu-list'].innerHTML.includes('Review &lt;agent>'), 'sessions do not mix in agents');
    assert(context.menuRows.some(function (row) { return row.kind === 'session' && row.agents === summary; }));
    elements['f-menu-search'].value = '@agents review';
    context.renderWindowMenu(data);
    assert(elements['menu-list'].innerHTML.includes('Review &lt;agent>'));
    assert.equal(context.menuRows.length, 1);
    assert.equal(context.menuRows[0].kind, 'agent');
    assert(!elements['menu-list'].innerHTML.includes('create-session'));
    context.lastMenuPick = 0;
    context.pickMenuResult();
    var command = sent[sent.length - 1];
    assert.equal(command.op, 'pane');
    assert.equal(command.id, pane);
    assert.equal(command.win, tab);
    assert.equal(command.session, session);
    assert.equal(command.expected_machine, backend);
    var count = sent.length;
    context.jumpToAgent('stale-host', session, tab, pane);
    assert.equal(sent.length, count, 'stale host must not select a same-ID pane');
    summary.state = 'working';
    context.renderTabs(data);
    assert(elements['window-tabs'].innerHTML.includes('[Working]'));
    data.agentPanes = [];
    data.tabAgents = {};
    context.renderTabs(data);
    assert(!elements['window-tabs'].innerHTML.includes('agent-badge'));
    elements['f-menu-search'].value = '@agents ';
    context.renderWindowMenu(data);
    assert.equal(elements['menu-list'].innerHTML,'','empty agent groups render no status message');
});
assert(context.agentBadge({state:'future',count:2}).includes('[Unknown 2]'));
console.log('WAF agent badges, filtering, exact pane selection and stale-host guard: ok');

var cross = {
    activeHost:'outbox',session:'same-session',
    panes:['P same-session same-tab same-pane 0 codex [80x24] 1'],
    hosts:[{id:'outbox',name:'Outbox'},{id:'devbox',name:'Devbox'},
        {id:'offline',name:'Offline'},{id:'loading',name:'Pending'},{id:'empty',name:'Empty'}],
    sessionCatalog:[]
};
['outbox','devbox'].forEach(function (machine) {
    cross.sessionCatalog.push({machine:machine,
        sessions:[{id:'same-session',name:machine+' project'}],
        tabs:[{id:'same-tab',name:machine+' review'}],
        agent_panes:[{id:'same-pane',session:'same-session',tab:'same-tab',
            agent:{name:'Review <agent>',state:'blocked'}}]});
});
cross.sessionCatalog.push({machine:'offline',unavailable:true,agent_panes:cross.sessionCatalog[0].agent_panes});
cross.sessionCatalog.push({machine:'loading',loading:true,agent_panes:[]});
cross.sessionCatalog.push({machine:'empty',agent_panes:[]});
context.lastData=cross;
elements['f-menu-search'].value='@agents ';
context.renderWindowMenu(cross);
assert.equal(context.menuRows.length,2,'same IDs on different hosts must not collide or duplicate');
assert.equal(context.menuRows.filter(function (r) { return r.active; }).length,1);
assert(elements['menu-list'].innerHTML.includes('Devbox / devbox project / devbox review'));
assert(!elements['menu-list'].innerHTML.includes('devbox review / same-pane'),
    'agent rows do not display backend pane IDs');
assert(!/Loading|Refreshing/.test(elements['menu-list'].innerHTML),
    'transient agent discovery states stay out of the dropdown');
assert(!/No agents detected|Unavailable/.test(elements['menu-list'].innerHTML),
    'empty and unavailable hosts add no status rows');
elements['f-menu-search'].value='@agents devbox blocked';
context.renderWindowMenu(cross);
assert.equal(context.menuRows.length,1);
var before=sent.length;
context.lastMenuPick=0;
context.pickMenuResult();
assert.equal(sent.length,before+1,'cross-host selection is one command, not host then pane');
assert.deepEqual(JSON.parse(JSON.stringify(sent[sent.length-1])),
    {op:'host_pane',id:'devbox',session:'same-session',tab:'same-tab',pane:'same-pane'});
assert.equal(context.awaitHost,'devbox');
cross.sessionCatalog[1].unavailable=true;
context.jumpToAgent('devbox','same-session','same-tab','same-pane');
assert.equal(sent.length,before+1,'a now-unavailable target must not be selected');
console.log('WAF cross-host agents: host-scoped names/IDs, offline/empty states, quiet loading and atomic routing OK');

elements['win-menu'] = { style: { display: 'none' }, innerHTML: '' };
elements['menu-title'] = { innerHTML: '' };
elements['session-chip'] = { className: '' };
elements['f-text'] = { blur: function () {} };
elements['menu-group-hosts'] = { className: '', setAttribute: function () {} };
elements['menu-group-sessions'] = { className: '', setAttribute: function () {} };
elements['menu-group-panes'] = { className: '', setAttribute: function () {} };
elements['menu-group-agents'] = { className: '', setAttribute: function () {} };
context.pokeScreen = function () {};

var jump = {
    connected: true, activeHost: 'devbox', session: 's1',
    hosts: [{ id: 'devbox', name: 'Devbox' }, { id: 'work', name: 'Work' }],
    sessionNames: { s1: 'api' }, tabNames: { t1: 'shell', t2: 'review' },
    panes: ['P s1 t1 p-shell 0 fish [80x24] 1'],
    sessionCatalog: [
        { machine: 'devbox', sessions: [{ id: 's1', name: 'api' }],
            tabs: [{ id: 't1', name: 'shell' }, { id: 't2', name: 'review' }],
            agent_panes: [
                { id: 'p-review', session: 's1', tab: 't2',
                    agent: { name: 'codex', state: 'blocked' } },
                { id: 'p-other', session: 's1', tab: 't2',
                    agent: { name: 'claude', state: 'working' } }
            ] },
        { machine: 'work', sessions: [{ id: 's2', name: 'svc' }],
            tabs: [{ id: 't9', name: 'ask' }],
            agent_panes: [
                { id: 'p-ask', session: 's2', tab: 't9',
                    agent: { name: 'grok', state: 'blocked' } }
            ] }
    ]
};
context.lastData = jump;
assert.equal(context.blockedAgentEntries(jump).length, 2);
assert.deepEqual(context.connectionChip(jump, 0),
    { text: 'Needs input \u00b7 2', cls: 'state-blocked', hidden: false, jump: true });
assert.equal(context.connectionChip({ connected: true }, 0).text, 'Live');
assert.equal(context.connectionChip({ connected: true }, 12).text, 'Live \u00b7 12 back');
assert.equal(context.connectionChip({ connected: false, copyMode: false }, 0).text, 'Offline');
assert.equal(context.connectionChip({ connected: true, copyMode: true }, 0).text, 'Copy');

sent = [];
assert(context.jumpToBlockedAgent());
assert.deepEqual(JSON.parse(JSON.stringify(sent.pop())),
    { op: 'pane', session: 's1', win: 't2', id: 'p-review', expected_machine: 'devbox' });
assert(!sent.some(function (op) { return op.op === 'text' || op.op === 'key'; }));

jump.panes = ['P s1 t2 p-review 0 codex [80x24] 1'];
jump.session = 's1';
sent = [];
assert(context.jumpToBlockedAgent());
assert.deepEqual(JSON.parse(JSON.stringify(sent.pop())),
    { op: 'host_pane', id: 'work', session: 's2', tab: 't9', pane: 'p-ask' });

jump.sessionCatalog[1].agent_panes = [];
jump.panes = ['P s1 t2 p-review 0 codex [80x24] 1'];
sent = [];
context.menuGroup = 'sessions';
assert(context.jumpToBlockedAgent());
assert.equal(context.menuGroup, 'agents',
    'the only blocked pane opens the Agents list instead of sending input');
assert(!sent.some(function (op) { return op.op === 'pane' || op.op === 'host_pane'; }));

assert.equal(context.blockedAgentEntries({ connected: true, sessionCatalog: [] }).length, 0);
context.lastData = { connected: true, sessionCatalog: [] };
assert(!context.jumpToBlockedAgent());
console.log('WAF jump-to-blocked-agent: chip, cycle, list fallback and no input OK');
