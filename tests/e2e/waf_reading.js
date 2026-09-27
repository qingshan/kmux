/* Reading mode hides typing chrome, skips the VKB, keeps paging/search,
 * and paints extra local history into the unused height without resizing
 * the remote pane. */
var assert = require("assert");
var fs = require("fs");
var vm = require("vm");

var markup = fs.readFileSync("kpm/waf/index.html", "utf8");
var css = fs.readFileSync("kpm/waf/style.css", "utf8");
var source = fs.readFileSync("kpm/waf/script.js", "utf8");
assert(/data-act="read"[^>]*id="btn-read"/.test(markup),
    "Tools must include a Read control");
assert(/id="tab-read"/.test(markup),
    "Read stays next to Settings so it cannot be clipped by the tab strip");
assert(/body\.reading \.kb-stabs/.test(css) && /body\.reading \.kb-toolbar/.test(css),
    "reading mode hides typing chrome and keeps the Scroll row");
assert(/body\.reading \.kb-chrome\s*\{[^}]*position:\s*fixed/.test(css),
    "the Scroll row stays pinned to the bottom");

var elements = {};
var sent = [];
var timers = [];
var doc = { body: { className: "" }, activeElement: null, listeners: {} };
doc.addEventListener = function () {};
function element(id, tag) {
    return elements[id] = {
        id: id,
        tagName: tag || "DIV",
        className: id === "tab-read" ? "tab" : "",
        innerHTML: "",
        offsetTop: 80,
        style: { display: "" },
        rows: [{ offsetHeight: 20 }],
        getElementsByTagName: function (tag) {
            return tag === "table" ? [{ rows: this.rows }] : [];
        },
        addEventListener: function () {},
        focus: function () { doc.activeElement = this; },
        blur: function () { if (doc.activeElement === this) { doc.activeElement = null; } }
    };
}
["kb-chrome", "btn-read", "tab-read", "f-text", "f-scroll-search", "term-area",
    "term", "window-tabs", "state", "view-main", "view-settings", "term-compose",
    "f-compose", "panel-scroll", "panel-prompt", "panel-tools", "kb-stabs"]
    .forEach(function (id) { element(id); });
elements["f-text"].tagName = "INPUT";
elements["f-scroll-search"].tagName = "INPUT";
elements["f-compose"].tagName = "TEXTAREA";
elements["kb-stabs"].getElementsByTagName = function () { return []; };

var context = {
    document: doc,
    window: { innerHeight: 80 + 20 * 40, addEventListener: function () {} },
    Date: Date,
    byId: function (id) { return elements[id] || null; },
    setTimeout: function (fn) { timers.push(fn); return timers.length; },
    clearTimeout: function () {},
    esc: function (text) {
        return String(text).replace(/&/g, "&amp;").replace(/</g, "&lt;");
    }
};
vm.createContext(context);
vm.runInContext(source, context);
context.setState = function () {};
context.sendCmd = function (op) { sent.push(op); };

assert.equal(context.viewportRows({ rows: 24 }), 24);
assert.equal(context.readingViewportRows(24), 40,
    "unused height becomes extra local rows at the current cell size");
assert.equal(context.viewportRows({ rows: 24, copyMode: true }), 24);
context.readingMode = true;
assert.equal(context.viewportRows({ rows: 24, copyMode: true }), 24,
    "copy mode keeps the captured 24-row overlay");
assert.equal(context.viewportRows({ rows: 24, altScreen: true }), 24,
    "alternate screens stay 24 rows");
assert.equal(context.viewportRows({ rows: 24 }), 40);
context.readingMode = false;

context.lastData = { cols: 80, rows: 24, cursorY: 23, cursorX: 0 };
var live = [];
var i;
for (i = 0; i < 24; i++) { live.push("live-" + i); }
var sb = ["new3", "new2", "new1", "new0", "older"];
var html = context.collectTermHtmlRows({ rows: 24, cols: 80 }, 0, live, [], sb);
assert.equal(html.length, 24, "live view stays 24 rows until reading mode");

context.readingMode = true;
html = context.collectTermHtmlRows({ rows: 24, cols: 80 }, 0, live, [], sb);
assert.equal(html.length, 40);
assert.equal(html[0], context.rowHtml("older", ".....", null, 0),
    "extra rows are the history just above live");
assert.equal(html[4], context.rowHtml("new3", "....", null, 4));
assert.equal(html[5], context.rowHtml("live-0", "", 0, 5),
    "the live 80x24 stays at the bottom of the view");
assert.equal(html[28], context.rowHtml("live-23", "", 23, 28));

html = context.collectTermHtmlRows({ rows: 24, cols: 80 }, 8, live, [], sb);
assert.equal(html.length, 40, "paged history uses the reading viewport, not 24");

context.readingMode = false;
doc.activeElement = elements["f-text"];
context.setReadingMode(true);
assert.equal(doc.body.className, "reading");
assert.equal(elements["panel-scroll"].style.display, "",
    "reading mode keeps the Scroll row");
assert.equal(elements["panel-tools"].style.display, "none");
assert.equal(elements["btn-read"].className, "active");
assert.equal(elements["tab-read"].className, "tab active");
assert.equal(doc.activeElement, null, "reading mode dismisses the capture field");
assert(sent.some(function (op) { return op.op === "scroll_page" && op.rows === 24; }),
    "physical page-turns must use the remote terminal height");
context.holdVkb();
assert.equal(doc.activeElement, null, "resize/holdVkb must not summon the keyboard");
context.focusTerminalKeyboard();
assert.equal(doc.activeElement, null, "a terminal tap must not type in reading mode");
assert.equal(context.readingLayoutPasses, 1);
timers.shift()();
assert.equal(context.readingLayoutPasses, 2, "remeasure after the keyboard drops");

doc.activeElement = elements["f-scroll-search"];
context.focusTerminalKeyboard();
assert.equal(doc.activeElement, null, "tapping the terminal dismisses search focus");

sent = [];
context.setReadingMode(false);
while (timers.length) { timers.shift()(); }
assert.equal(doc.body.className, "");
assert.equal(elements["btn-read"].className, "");
assert.equal(elements["tab-read"].className, "tab");
assert.equal(doc.activeElement, elements["f-text"], "leaving reading restores the keyboard");
assert(sent.some(function (op) { return op.op === "scroll_page" && op.rows === 24; }));

console.log("waf reading: ok");
