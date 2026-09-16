/* Exercise WAF routing and labels using opaque IDs from both backends. */
var assert = require('assert');
var fs = require('fs');
var vm = require('vm');
var elements = { 'window-tabs': { innerHTML: '' }, 'f-scroll-search': { value: 'needle' } };
var sent = [];
var context = {
    document: { addEventListener: function () {} },
    setTimeout: function () {}, clearTimeout: function () {}, Date: Date,
    byId: function (id) { return elements[id] || null; },
    esc: function (s) { return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;'); },
    sendJsonCmd: function (service, op) { sent.push(op); return true; }
};
vm.createContext(context);
vm.runInContext(fs.readFileSync('kpm/waf/script.js','utf8'), context);
['tmux', 'herdr'].forEach(function (backend) {
    var session = backend === 'tmux' ? '$1' : 'w1';
    var tab = backend === 'tmux' ? '@2' : 'w1:t2';
    var pane = backend === 'tmux' ? '%3' : 'w1:p3';
    var data = {
        activeHost: backend, session: session, connected: true,
        windows: ['W '+session+' '+tab+' Long_label * [80x24]'],
        panes: ['P '+session+' '+tab+' '+pane+' 0 shell [80x24] 1'],
        sessionNames: {}, tabNames: {}, hosts: []
    };
    data.sessionNames[session] = 'Work space';
    data.tabNames[tab] = 'Long label <test>';
    context.lastData = data;
    context.renderTabs(data);
    assert(elements['window-tabs'].innerHTML.includes('Work space'));
    assert(elements['window-tabs'].innerHTML.includes('Long label &lt;test>'));
    context.sendCmd({ op:'text', text:'hello' });
    assert.equal(sent[sent.length-1].expected_pane,pane);
    assert.equal(sent[sent.length-1].expected_machine,backend);
    data.copyMode = true;
    context.searchScrollback('back');
    assert.equal(sent[sent.length-1].op,'copy_search');
    assert.equal(sent[sent.length-1].query,'needle');
    assert.equal(sent[sent.length-1].backwards,true);
});
console.log('WAF unified routing: ok');

elements['menu-list'] = { innerHTML: '' };
elements['f-menu-search'] = { value: '' };
var menuData = {
    activeHost: 'outbox', session: 'same-id', windows: ['W same-id @1 Shell *'], sessions: [],
    hosts: [{id:'outbox',name:'Outbox'}, {id:'devbox',name:'Devbox'}, {id:'offline',name:'Offline'}],
    sessionCatalog: [
        {machine:'outbox',sessions:[{id:'same-id',name:'Main'}]},
        {machine:'devbox',sessions:[{id:'same-id',name:'Work <project>'}]},
        {machine:'offline',sessions:[],unavailable:true}
    ]
};
context.renderWindowMenu(menuData);
assert(elements['menu-list'].innerHTML.includes('Outbox / Main'));
assert(elements['menu-list'].innerHTML.includes('Devbox / Work &lt;project>'));
assert(!elements['menu-list'].innerHTML.includes('Unavailable'));
assert(elements['menu-list'].innerHTML.includes('data-host="devbox"'));
function menuRow(kind, host) {
    return elements['menu-list'].innerHTML.split('</div>').filter(function (row) {
        return row.includes('data-kind="' + kind + '"') &&
            row.includes('data-host="' + host + '"');
    })[0];
}
assert(menuRow('host-session', 'outbox').includes(context.ICON_SESSION), 'active tmux session has an icon');
assert(menuRow('host-session', 'outbox').includes(context.ICON_ACTIVE), 'session icon preserves the active marker');
assert(menuRow('host-session', 'devbox').includes(context.ICON_SESSION), 'inactive Herdr workspace has an icon');
assert(!menuRow('host-session', 'devbox').includes(context.ICON_ACTIVE));
assert(!menuRow('window', 'outbox'), 'session group must not mix in window shortcuts');
elements['f-menu-search'].value = 'dev work';
context.renderWindowMenu(menuData);
assert(!elements['menu-list'].innerHTML.includes('Outbox / Main'));
assert.equal(context.menuRows[0].host, 'devbox');
assert(menuRow('host-session', 'devbox').includes(context.ICON_SESSION), 'filtered sessions retain their icon');
context.beginPaneSwitch = function () {};
context.closeWinMenu = function () {};
context.showView = function () {};
context.pickMenuResult();
assert.equal(sent[sent.length-1].op, 'host_session');
assert.equal(sent[sent.length-1].id, 'devbox');
assert.equal(sent[sent.length-1].session, 'same-id');
assert.equal(context.awaitHost, 'devbox');
elements['f-menu-search'].value = '';
menuData.sessionCatalog[2] = {machine:'offline',sessions:[],loading:true};
context.renderWindowMenu(menuData);
assert(!/Loading|Refreshing/.test(elements['menu-list'].innerHTML),
    'transient catalog states stay out of the dropdown');
assert(!/No sessions|Unavailable/.test(elements['menu-list'].innerHTML),
    'empty and unavailable hosts do not add status rows');
console.log('WAF cross-host sessions: ok');

var clock = 10000;
var nextTick;
var tickDelay;
var lists = 0;
context.Date = { now: function () { return clock; } };
context.lastData = { configured: true, eventUpdates: true };
context.lastInputAt = 0;
context.fetchStatusJson = function () {};
context.sendCmd = function (op) { if (op.op === 'list') { lists++; } };
context.setTimeout = function (fn, ms) { nextTick = fn; tickDelay = ms; return 1; };
context.startAutoRefresh();
assert.equal(tickDelay, 500);
for (var tick = 0; tick < 6; tick++) { clock += 500; nextTick(); }
assert.equal(lists, 2, 'fast local refresh must not multiply proxy list requests');
context.lastData.eventUpdates = false;
nextTick();
assert.equal(tickDelay, 2500);
console.log('WAF event refresh and request throttling: ok');
