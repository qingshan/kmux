/* All overlay open/close paths use the real WAF functions and ES5 DOM stubs. */
var assert = require('assert');
var fs = require('fs');
var vm = require('vm');
var markup = fs.readFileSync('kpm/waf/index.html', 'utf8');
assert(/<button[^>]*data-act="files"[^>]*id="btn-files"[^>]*>Files<\/button>/.test(markup));
var elements = {};
var timers = [];
var sent = [];
var doc = { activeElement: null, listeners: {} };
doc.addEventListener = function (kind, fn) {
    (this.listeners[kind] || (this.listeners[kind] = [])).push(fn);
};
function element(id, parent) {
    return elements[id] = { id:id, parentNode:parent || doc, tagName:'DIV', value:'', innerHTML:'',
        style:{display:'none'}, listeners:{}, addEventListener:doc.addEventListener,
        setAttribute:function (key,value) { this[key]=value; },
        focus:function () { doc.activeElement=this; },
        blur:function () { if (doc.activeElement===this) { doc.activeElement=null; } } };
}
markup.replace(/\bid="([^"]+)"/g, function (_, id) { element(id); });
elements['f-compose'].parentNode = elements['term-compose'];
elements['f-compose'].tagName = 'TEXTAREA';
elements['f-proxy'].parentNode = elements['view-settings'];
elements['f-proxy'].tagName = 'INPUT';
elements['f-socks5'].parentNode = elements['view-settings'];
elements['f-socks5'].tagName = 'INPUT';
var context = { document:doc, Date:Date,
    byId:function (id) { return elements[id] || null; },
    setTimeout:function (fn) { timers.push(fn); return timers.length; }, clearTimeout:function () {},
    esc:function (text) { return String(text).replace(/&/g,'&amp;').replace(/</g,'&lt;'); }
};
vm.createContext(context);
vm.runInContext(fs.readFileSync('kpm/waf/script.js','utf8') + '\n' +
    fs.readFileSync('kpm/waf/snippet-overlay.js','utf8'), context);
context.sendCmd = function (op) { sent.push(op); };
context.pokeScreen = function () {};
context.clearMods = function () {};
context.wireOverlayChrome();
context.wireCompose();
function flush() { while (timers.length) { timers.shift()(); } }
function fire(target, kind, key) {
    var ev = {target:target,keyCode:key,stopped:false,prevented:false,
        preventDefault:function () { this.prevented=true; },
        stopPropagation:function () { this.stopped=true; }};
    for (var node=target; node && !ev.stopped; node=node.parentNode) {
        (node.listeners[kind] || []).forEach(function (fn) { fn.call(node,ev); });
    }
    return ev;
}
var openers = {
    'win-menu':function () { context.openWinMenu(); },
    'act-menu':function () { context.openActMenu(); },
    'file-picker':function () { context.openFilePicker(); },
    'paste-pop':function () { context.showPastePop(); },
    'snippet-pop':function () { context.showSnippetPop(); },
    'term-compose':function () { context.showCompose('draft'); },
    'view-settings':function () { context.showView('settings'); elements['f-proxy'].focus(); },
    'err-pop':function () { context.lastErrorText='<unsafe>'; context.toggleErrPop(); }
};
function visible() { return Object.keys(openers).filter(function (id) { return context.overlayVisible(id); }); }
Object.keys(openers).forEach(function (id) {
    assert(new RegExp('id="'+id+'"[^>]*class="[^"]*overlay[^"]*"[^>]*role="dialog"[^>]*aria-labelledby=').test(markup));
    var closeId = id==='term-compose' ? 'btn-compose-cancel' : 'close-'+id;
    assert(new RegExp('id="'+closeId+'"[^>]*aria-label=').test(markup));
    context.prepareOverlay('');
    openers[id](); flush();
    assert.deepEqual(visible(),[id]);
    var icon = element('icon-'+id,elements[closeId]);
    fire(icon,'mousedown'); flush();
    assert.deepEqual(visible(),[],id+' close icon');
    assert.strictEqual(doc.activeElement,elements['f-text']);
    openers[id](); flush();
    fire(doc,'keydown',27); flush();
    assert.deepEqual(visible(),[],id+' Escape');
    assert.strictEqual(doc.activeElement,elements['f-text']);
    Object.keys(openers).forEach(function (next) {
        if (next===id) { return; }
        context.prepareOverlay(''); openers[id](); openers[next](); flush();
        assert.deepEqual(visible(),[next],id+' -> '+next);
    });
});
context.prepareOverlay('');
context.showCompose('keep draft'); flush();
context.setError('<network failure>');
assert.deepEqual(visible(),['term-compose']);
assert.equal(elements['f-compose'].value,'keep draft');
assert.equal(elements['error-message'].innerHTML,'&lt;network failure>');
context.cancelCompose(); flush();
context.toggleErrPop();
assert.deepEqual(visible(),['err-pop']);
assert.equal(elements['err-pop'].innerHTML,'','error updates must not replace header/close button');
context.sendWithMods('key','Escape'); flush();
assert.deepEqual(visible(),[]);
assert(!sent.some(function (op) { return op.op==='key' || op.op==='text'; }), 'dismissal never types into terminal');
console.log('WAF overlays: shared chrome, nested close icons, Escape, focus, 56 transitions and safe errors OK');

// CSS2 em sizes depend on ancestry: the terminal has an extra 0.9em parent.
// Keep all overlay descendants at the overlay scale, without nested shrinking.
var css = fs.readFileSync('kpm/waf/style.css','utf8').replace(/\/\*[\s\S]*?\*\//g,'');
assert(/\.overlay \.overlay-icon\s*\{[^}]*width: 2em;\s*height: 2em;/.test(css),
    'Close, Send and Save share the same compact square dimensions');
assert(!/\.overlay \.overlay-primary\s*\{[^}]*(?:width|height):/.test(css),
    'primary actions must not override shared button dimensions');
assert(!css.includes('.overlay .overlay-header .overlay-icon'),
    'header buttons must use the shared rule, not a separate size');
function fontScale(selector) {
    var scale;
    css.replace(/([^{}]+)\{([^{}]*)\}/g,function (_, selectors, body) {
        if (selectors.split(',').map(function (s) { return s.trim(); }).indexOf(selector)<0) { return; }
        var match = /(?:^|;)\s*font-size:\s*([0-9.]+)em\s*(?:;|$)/.exec(body);
        if (match) { scale=parseFloat(match[1]); }
    });
    return scale;
}
assert(Math.abs(fontScale('.overlay') - fontScale('#view-main') * fontScale('.term')) < 0.00001,
    'overlay and terminal effective font sizes must match');
assert(!/padding-left:\s*0\.15em[\s\S]{0,80}#view-settings|#view-settings[\s\S]{0,80}padding-left:\s*0\.15em/.test(css),
    'Settings must use shared overlay padding, not the page chrome inset');
['.overlay-title','.overlay .overlay-icon','.overlay .search-field','.overlay input',
 '.overlay textarea','.overlay .search-field input','.overlay-field label','.win-menu-item',
 '.paste-item','.overlay .agent-badge','.overlay .empty','.paste-empty','.overlay-message',
 '.overlay .note','.compose-hint','.overlay .menu-group'].forEach(function (selector) {
    assert.equal(fontScale(selector),1,selector+' must not rescale popup text');
});
console.log('WAF overlay typography: all text matches terminal effective font size OK');
