/* Group tabs filter navigation locally; selecting a row is the only action. */
var assert = require('assert');
var fs = require('fs');
var vm = require('vm');
var elements = {};
var timers = [];
var sent = [];
var doc = {activeElement:null,addEventListener:function () {}};
function element(id) {
    return elements[id] = {id:id,value:'',innerHTML:'',style:{display:'none'},listeners:{},attrs:{},
        setAttribute:function (k,v) { this.attrs[k]=v; },
        getAttribute:function (k) { return this.attrs[k] || null; },
        addEventListener:function (kind,fn) { this.listeners[kind]=fn; },
        focus:function () { doc.activeElement=this; },
        blur:function () { if (doc.activeElement===this) { doc.activeElement=null; } }};
}
fs.readFileSync('kpm/waf/index.html','utf8').replace(/\bid="([^"]+)"/g,function (_,id) { element(id); });
assert.equal(elements['btn-agents'],undefined,'Agents is available in the switcher, not Tools');
assert(elements['tab-settings'],'Settings is available in the main top bar');
assert.equal(elements['menu-settings'],undefined,'Settings is not duplicated in the dropdown');
['hosts','sessions','panes','agents'].forEach(function (name) {
    var el=elements['menu-group-'+name];
    el.parentNode=elements['menu-groups'];
    el.attrs['data-menu-group']=name;
});
var context={document:doc,Date:Date,
    byId:function (id) { return elements[id] || null; },
    esc:function (s) { return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;'); },
    setTimeout:function (fn) { timers.push(fn); return timers.length; },clearTimeout:function () {}};
vm.createContext(context);
vm.runInContext(fs.readFileSync('kpm/waf/script.js','utf8'),context);
context.clearMods=function () {};
context.sendCmd=function (op) { sent.push(op); };
context.pokeScreen=function () {};
context.beginPaneSwitch=function () {};
context.showView=function () {};
var data={activeHost:'outbox',session:'s1',hosts:[{id:'outbox',name:'Outbox'},{id:'devbox',name:'Devbox'}],
    sessions:['S s1 1'],sessionNames:{s1:'Main'},tabNames:{t1:'Shell'},
    windows:['W s1 t1 Shell *'],panes:['P s1 t1 p1 0 sh [80x24] 1','P s1 t1 p2 1 codex [80x24] 0'],
    agentPanes:[{id:'p2',session:'s1',tab:'t1',agent:{name:'Local reviewer',state:'working'}}],
    sessionCatalog:[{machine:'outbox',sessions:[{id:'s1',name:'Main'}]},
        {machine:'devbox',sessions:[{id:'s1',name:'Remote'}],tabs:[{id:'t1',session:'s1',name:'Review'}],
            panes:[{id:'p2',session:'s1',tab:'t1',command:'codex',cwd:'/tmp'},
                {id:'p3',session:'s1',tab:'t1',command:'sh',cwd:'/tmp'}],
            agent_panes:[{id:'p2',session:'s1',tab:'t1',agent:{name:'Remote reviewer',state:'blocked'}}]}]};
context.lastData=data;
context.wireMenuGroups();
function flush() { while(timers.length) { timers.shift()(); } }
function tab(name) {
    elements['menu-groups'].listeners.mousedown({target:{parentNode:elements['menu-group-'+name]},
        preventDefault:function () {},stopPropagation:function () {}});
    flush();
    assert.equal(context.menuGroup,name);
    ['hosts','sessions','panes','agents'].forEach(function (other) {
        assert.equal(elements['menu-group-'+other].attrs['aria-selected'],other===name?'true':'false');
    });
}
function search(text) { elements['f-menu-search'].value=text; context.renderWindowMenu(data); }
context.openWinMenu(); flush();
assert.equal(context.menuGroup,'sessions');
assert(context.menuRows.every(function (r) { return r.kind==='session'; }));
assert.equal(sent.length,1);
sent=[];
tab('hosts');
assert.equal(context.menuRows.length,2);
assert(context.menuRows.every(function (r) { return r.kind==='host'; }));
assert(elements['menu-list'].innerHTML.indexOf(context.ICON_SERVER) !== -1);
assert(elements['menu-list'].innerHTML.indexOf(context.ICON_SESSION) === -1);
tab('sessions');
assert(elements['menu-list'].innerHTML.indexOf(context.ICON_SESSION) !== -1);
assert(elements['menu-list'].innerHTML.indexOf(context.ICON_SERVER) === -1);
tab('panes');
assert.equal(context.menuRows.length,4);
assert(context.menuRows.every(function (r) { return r.kind==='pane'; }));
assert(elements['menu-list'].innerHTML.indexOf(context.ICON_PANE) !== -1);
assert(elements['menu-list'].innerHTML.indexOf(context.ICON_SERVER) === -1);
assert.equal(elements['f-menu-search'].attrs.placeholder,'Search panes across hosts...');
assert(!/Loading|Refreshing/.test(elements['menu-list'].innerHTML));
search('devbox Remote Review codex');
assert.equal(context.menuRows.length,1);
assert(elements['menu-list'].innerHTML.includes('Devbox / Remote / Review — codex'));
assert(!elements['menu-list'].innerHTML.includes('Review / p2'));
context.lastMenuPick=0; context.pickMenuResult();
assert.deepEqual(JSON.parse(JSON.stringify(sent.pop())),{op:'host_pane',id:'devbox',session:'s1',tab:'t1',pane:'p2'});
search('codex');
tab('agents');
assert.equal(elements['f-menu-search'].value,'');
assert.equal(context.menuRows.length,2);
assert(context.menuRows.every(function (r) { return r.kind==='agent'; }));
assert(elements['menu-list'].innerHTML.indexOf(context.ICON_AGENT) !== -1);
assert(elements['menu-list'].innerHTML.indexOf(context.ICON_PANE) === -1);
search('devbox blocked');
tab('panes');
assert.equal(elements['f-menu-search'].value,'codex','per-group searches are retained');
assert.equal(context.menuRows.length,2);
assert.equal(sent.length,0,'group switches and searches must not affect the terminal');
search('outbox codex');
assert.equal(context.menuRows.length,1);
context.lastMenuPick=0; context.pickMenuResult();
assert.deepEqual(JSON.parse(JSON.stringify(sent.pop())),{op:'pane',session:'s1',win:'t1',id:'p2',expected_machine:'outbox'});
context.menuOpen=true;
tab('agents');
assert.equal(elements['f-menu-search'].value,'devbox blocked');
context.lastMenuPick=0; context.pickMenuResult();
assert.deepEqual(JSON.parse(JSON.stringify(sent.pop())),{op:'host_pane',id:'devbox',session:'s1',tab:'t1',pane:'p2'});
context.menuOpen=true;
tab('hosts'); search('devbox');
context.lastMenuPick=0; context.pickMenuResult();
assert.equal(sent.pop().op,'host');
tab('panes'); search('missing');
assert.equal(context.menuRows.length,0,'non-session groups never offer session creation');
assert(elements['menu-list'].innerHTML.includes('No matches'));
tab('sessions'); search('new-session');
assert(context.menuRows.some(function (r) { return r.create; }));
tab('agents');
context.renderWindowMenu(data);
assert.equal(context.menuGroup,'agents','status refresh does not reset the selected group');
elements['f-menu-search'].value='stale query';
context.openWinMenu(); flush();
assert.equal(context.menuGroup,'sessions','session chip defaults to sessions');
assert.equal(elements['f-menu-search'].value,'','reopening clears stale queries');
console.log('WAF navigation groups: isolated rows, tab taps, scoped search, exact selection and refresh persistence OK');
