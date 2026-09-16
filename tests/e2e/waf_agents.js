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
