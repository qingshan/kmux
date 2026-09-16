/* Exercise real compose handlers and scroll synchronization, using ES5 DOM stubs. */
var assert = require('assert');
var fs = require('fs');
var vm = require('vm');
var timers = [];
var sent = [];
var elements = {};
var doc = { listeners: {}, activeElement: null };
doc.addEventListener = function (kind, fn) {
    if (!this.listeners[kind]) { this.listeners[kind] = []; }
    this.listeners[kind].push(fn);
};
function element(id, parent, tag) {
    var el = { id: id, parentNode: parent || doc, tagName: tag || 'DIV',
        style: { display: 'none' }, value: '', listeners: {},
        addEventListener: doc.addEventListener,
        focus: function () { doc.activeElement = this; },
        blur: function () { if (doc.activeElement === this) { doc.activeElement = null; } }
    };
    elements[id] = el;
    return el;
}
var pop = element('term-compose');
var field = element('f-compose', pop, 'TEXTAREA');
var send = element('btn-compose-send', pop, 'BUTTON');
var cancel = element('btn-compose-cancel', pop, 'BUTTON');
var sendIcon = element('send-icon', send, 'SPAN');
var closeIcon = element('close-icon', cancel, 'SPAN');
var capture = element('f-text', doc, 'INPUT');
var term = element('term');
var context = { document: doc, Date: Date,
    byId: function (id) { return elements[id] || null; },
    setTimeout: function (fn, ms) { timers.push(fn); return timers.length; },
    clearTimeout: function () {}
};
vm.createContext(context);
vm.runInContext(fs.readFileSync('kpm/waf/script.js', 'utf8'), context);
['hideSnippetPop','hidePastePop','hideErrPop','closeWinMenu','closeActMenu']
    .forEach(function (name) { context[name] = function () {}; });
context.showView = function () { context.holdVkb(); };
context.sendCmd = function (op) { sent.push(op); };
context.afterPressFlash = function (fn) { timers.push(fn); };
function flush() { while (timers.length) { timers.shift()(); } }
function fire(target, kind, key, shift, y) {
    var event = { target: target, keyCode: key, clientY: y || 0, touches: [{clientY:y || 0}], stopped: false,
        shiftKey: !!shift, prevented: false,
        preventDefault: function () { this.prevented = true; }, stopPropagation: function () { this.stopped = true; } };
    var node = target;
    while (node && !event.stopped) {
        (node.listeners[kind] || []).forEach(function (fn) { fn.call(node, event); });
        node = node.parentNode;
    }
    return event;
}
context.wireCompose();
context.wireTerminalDrag();

fire(term, 'mousedown');
fire(term, 'mousedown');
flush();
assert(context.composeVisible(), 'opening double-tap must not trigger outside-close');
assert.strictEqual(doc.activeElement, field, 'first tap must not steal compose focus');
assert(context.otherInputFocused(), 'textarea must retain editing focus');
field.value = 'do not execute';
fire(closeIcon, 'mousedown');
flush();
assert(!context.composeVisible());
assert.strictEqual(doc.activeElement, capture);
assert.equal(sent.length, 0);

context.showCompose('escape draft'); flush();
fire(field, 'keydown', 27); flush();
assert(!context.composeVisible());
assert.equal(sent.length, 0);
context.showCompose('toolbar escape draft'); flush();
context.sendWithMods('key','Escape'); flush();
assert(!context.composeVisible());
assert.equal(sent.length, 0);

context.showCompose('outside draft'); flush();
fire(term, 'mousedown'); flush();
assert(!context.composeVisible());
assert.equal(sent.length, 0);

context.showCompose('old draft'); flush();
fire(send, 'mousedown');
fire(cancel, 'mousedown');
context.showCompose('new draft'); flush();
assert.equal(sent.length, 0, 'a cancelled Send callback must not send a new dialog draft');
fire(sendIcon, 'mousedown'); fire(sendIcon, 'mousedown'); flush();
assert.equal(sent.filter(function (op) { return op.op === 'text'; }).length, 1);
assert(!context.composeVisible());
sent = [];
context.showCompose('with enter'); flush();
fire(field, 'keydown', 13); fire(field, 'keydown', 13); flush();
assert.equal(sent.filter(function (op) { return op.op === 'text'; }).length, 1);
assert.equal(sent.filter(function (op) { return op.op === 'key' && op.key === 'Enter'; }).length, 1);

sent = [];
context.showCompose('first line'); flush();
var newline = fire(field, 'keydown', 13, true);
assert(!newline.prevented, 'Shift+Enter must preserve the native textarea newline');
assert(context.composeVisible());
assert.equal(sent.length, 0, 'editing a newline must not send remote input');
field.value = 'first line\nsecond line λ'; // Browser inserts the newline, not the key handler.
fire(sendIcon, 'mousedown'); fire(sendIcon, 'mousedown'); flush();
assert.deepEqual(sent.filter(function (op) { return op.op === 'text'; }),
    [{op:'text',text:'first line\nsecond line λ',paste:true}]);
assert.equal(sent.filter(function (op) { return op.op === 'key'; }).length, 0,
    'paper-plane sends the draft without appending Enter');

var markup = fs.readFileSync('kpm/waf/index.html', 'utf8');
assert(/<textarea[^>]*id="f-compose"[^>]*rows="5"/.test(markup));
assert(/id="btn-compose-send"[^>]*aria-label="Send text without Enter"[^>]*><span[^>]*>&#xF1D8;/.test(markup));
assert(/id="btn-compose-cancel"[^>]*aria-label="Close without sending"[^>]*><span[^>]*>&#xF00D;/.test(markup));

var clipboardOpens = 0;
var scrollOffsets = [];
context.showPastePop = function () { clipboardOpens++; };
context.scrollToOffset = function (offset) { scrollOffsets.push(offset); };
context.lastData = { rows:24, scrollback:new Array(80), copyMode:false };
context.viewOffset = 0;
['touchstart','mousedown'].forEach(function (kind) {
    context.lastTermTapAt = 0;
    fire(term,kind,null,false,100);
    flush(); // Run every timer, including any accidental long-press timer.
    fire(doc,'mousemove',null,false,105); flush(); // A slow start below drag threshold.
    fire(doc,'mousemove',null,false,130); flush();
});
assert.equal(clipboardOpens,0,'holding or slowly scrolling must never open clipboard history');
assert.deepEqual(scrollOffsets,[24,24],'touch and mouse scrolling still page through history');

context.pendingRestore = null;
context.switchPending = false;
context.dragActive = false;
context.viewOffset = 48;
context.scrollAckUntil = Date.now() + 10000;
context.syncDaemonScroll({scrollOffset:24,scrollRevision:100});
assert.equal(context.viewOffset, 24);
context.syncDaemonScroll({scrollOffset:0,scrollRevision:101});
assert.equal(context.viewOffset, 0, 'physical down must reach live');
context.syncDaemonScroll({scrollOffset:48,scrollRevision:100});
assert.equal(context.viewOffset, 0, 'older poll must not undo button scrolling');
context.syncDaemonScroll({scrollOffset:24,scrollRevision:102});
assert.equal(context.viewOffset, 24);
context.syncDaemonScroll({scrollOffset:0,scrollRevision:102});
assert.equal(context.viewOffset, 24, 'unmarked zero must not undo touch history');
context.syncDaemonScroll({scrollOffset:0,scrollRevision:103,copyMode:true});
assert.equal(context.viewOffset, 24, 'copy mode owns its own cursor');
console.log('WAF compose cancellation/focus/deduplication and physical scrolling: ok');
