/* Sustained input must keep refreshing; cached rows must retain all styling. */
var assert = require('assert');
var fs = require('fs');
var vm = require('vm');
var now = 0;
var nextId = 0;
var timers = {};
var reads = 0;
var context = {
    document: {addEventListener:function () {}},
    setTimeout:function (fn, ms) { var id = ++nextId; timers[id] = {fn:fn, at:now + ms}; return id; },
    clearTimeout:function (id) { delete timers[id]; },
    esc:function (s) { return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;'); }
};
vm.createContext(context);
vm.runInContext(fs.readFileSync('kpm/waf/script.js', 'utf8'), context);
context.fetchStatusJson = function () { reads++; };
function advance(until) {
    while (true) {
        var id = Object.keys(timers).sort(function (a, b) { return timers[a].at - timers[b].at; })[0];
        if (!id || timers[id].at > until) { break; }
        var timer = timers[id]; delete timers[id]; now = timer.at; timer.fn();
    }
    now = until;
}
for (var i = 0; i < 25; i++) {
    context.pokeScreen();
    assert.equal(Object.keys(timers).length, 1, 'one pending input refresh');
    advance(now + 40);
}
assert(reads >= 5, 'typing continuously must not postpone every refresh');
advance(now + 2500);
assert.equal(Object.keys(timers).length, 0, 'input refresh stops after the burst');

context.lastData = {cols:80, cursorX:0, cursorY:0};
var cells = 0;
var termTd = context.termTd;
context.termTd = function () { cells++; return termTd.apply(context, arguments); };
var first = context.rowHtml('hello', '', 0, 0);
var before = cells;
assert.equal(context.rowHtml('hello', '', 0, 0), first);
assert.equal(cells, before, 'unchanged rows allocate no new cell markup');
context.lastData.cursorX = 1;
assert.notEqual(context.rowHtml('hello', '', 0, 0), first, 'cursor position invalidates the row');
context.viewOffset = 1;
var history = context.rowHtml('hello', '', 0, 0);
assert(!/class="cur"/.test(history), 'history hides the cursor');
context.searchNeedle = 'hello';
assert.notEqual(context.rowHtml('hello', '', 0, 0), history, 'search highlights invalidate the row');
context.searchNeedle = '';
assert.notEqual(context.rowHtml('hello', 'iiiii', 0, 0), history, 'attributes invalidate the row');
assert.notEqual(context.rowHtml('world', '', 0, 0), history, 'text changes invalidate the row');
var prompt = context.rowHtml('❯ hello', '', null, 4);
context.cmdHistIdx = 4;
assert.notEqual(context.rowHtml('❯ hello', '', null, 4), prompt, 'command selection invalidates the row');
context.lastData.cols = 40;
assert.notEqual(context.rowHtml('hello', '', 0, 0), history, 'column count invalidates the row');
for (i = 0; i < 300; i++) { context.rowHtml('line ' + i, '', null, i); }
assert.equal(Object.keys(context.rowCache).length, 144, 'scrolling keeps cache memory bounded');
console.log('WAF responsiveness: sustained input refresh and bounded row caching OK');
