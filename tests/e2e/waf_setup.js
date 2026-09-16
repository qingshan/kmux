/* First-run settings prompt and non-secret settings synchronization. */
var assert = require('assert');
var fs = require('fs');
var vm = require('vm');
var markup = fs.readFileSync('kpm/waf/index.html', 'utf8');
var elements = {};
var doc = {activeElement:null,addEventListener:function () {}};
markup.replace(/\bid="([^"]+)"/g, function (_, id) {
    elements[id] = {id:id,value:'',style:{display:'none'},listeners:{},
        addEventListener:function (kind, fn) { this.listeners[kind]=fn; },
        focus:function () { doc.activeElement=this; }};
});
var context = {document:doc,Date:Date,byId:function (id) { return elements[id] || null; },
    setTimeout:function () { return 1; },clearTimeout:function () {},
    setState:function () {},setError:function () {},paintFullGrid:function () {},
    showView:function (name) { this.shownView=name; },
    esc:function (text) { return String(text).replace(/&/g,'&amp;').replace(/</g,'&lt;'); }};
vm.createContext(context);
vm.runInContext(fs.readFileSync('kpm/waf/script.js','utf8'),context);
context.setState=function () {};
context.setError=function () {};
context.paintFullGrid=function () {};
context.clearMods=function () {};
context.render({configured:false,proxyUrl:'https://kmux.qingshan.dev',socks5:'',rows:24,cols:80});
assert.equal(elements['view-settings'].style.display,'','missing credentials open Settings on startup');
assert.equal(elements['f-proxy'].value,'https://kmux.qingshan.dev');
assert.equal(elements['f-socks5'].value,'');
assert.equal(elements['f-token'].value,'','the saved token is never exposed to WAF status');
elements['view-settings'].style.display='none';
context.render({configured:false,proxyUrl:'https://kmux.qingshan.dev',socks5:'',rows:24,cols:80});
assert.equal(elements['view-settings'].style.display,'none','startup prompt is shown once per app launch');
context.settingsDirty=true;
elements['f-proxy'].value='https://edited.example';
context.syncSettings({proxyUrl:'https://kmux.qingshan.dev',socks5:'127.0.0.1:1055'});
assert.equal(elements['f-proxy'].value,'https://edited.example','polling does not overwrite a draft');
assert.equal(elements['f-socks5'].value,'','polling does not overwrite a draft');
assert(/https:\/\/kmux\.qingshan\.dev/.test(markup),'settings show the public HTTPS endpoint by default');
assert(/id="btn-diagnose"/.test(markup) && /Diagnose/.test(markup),
    'settings include a Diagnose button');
assert(elements['diagnose-report'], 'settings include a diagnose report');
assert(!/overlay-list/.test(markup.match(/id="diagnose-report"[^>]*>/)[0]),
    'diagnose status is not an overlay list');
context.paintDiagnose({running:true, proxy:{ok:true, detail:'checking…'}, hosts:[]});
assert.equal(elements['diagnose-report'].style.display, '');
assert(/Checking/.test(elements['diagnose-report'].innerHTML));
context.paintDiagnose({running:false, proxy:{ok:true, detail:'API v1, 2 machines'},
    hosts:[{id:'devbox', name:'devbox', backend:'tmux', ok:true,
        ssh:{ok:true, detail:'local qingshan'},
        service:{ok:true, detail:'2 sessions: api, build'}},
        {id:'lab', name:'lab', backend:'herdr', ok:false,
        ssh:{ok:false, detail:'Connection timed out'},
        service:{ok:false, detail:'skipped (host unreachable)'}}]});
assert(/devbox/.test(elements['diagnose-report'].innerHTML));
assert(/2 sessions: api, build/.test(elements['diagnose-report'].innerHTML));
assert(/fail/.test(elements['diagnose-report'].innerHTML));
assert(/Connection timed out/.test(elements['diagnose-report'].innerHTML));
assert(!/diagnose-row/.test(elements['diagnose-report'].innerHTML));
assert.equal(elements['view-settings'].style.display, 'none',
    'diagnose rendering does not open or close Settings');
console.log('WAF setup: first-run settings, direct HTTPS defaults and safe draft synchronization OK');
