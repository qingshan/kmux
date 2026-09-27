/* File picks load into compose, like snippets. Directories still navigate. */
var assert = require("assert");
var fs = require("fs");
var vm = require("vm");

var markup = fs.readFileSync("kpm/waf/index.html", "utf8");
assert(/id="files-title"[^>]*>Choose file</.test(markup));
assert(/data-act="files"[^>]*title="Choose file for compose"/.test(markup));

var elements = {};
var sent = [];
var timers = [];
var doc = { body: { className: "" }, activeElement: null, listeners: {} };
doc.addEventListener = function () {};
function element(id, tag) {
    return elements[id] = {
        id: id,
        tagName: tag || "DIV",
        className: "",
        value: "",
        innerHTML: "",
        style: { display: id === "file-picker" ? "" : "none" },
        addEventListener: function () {},
        focus: function () { doc.activeElement = this; },
        blur: function () { if (doc.activeElement === this) { doc.activeElement = null; } }
    };
}
["file-picker", "btn-files", "f-file-search", "file-cwd", "file-list", "f-text",
    "term-compose", "f-compose", "view-main", "view-settings", "tab-settings",
    "kb-chrome"]
    .forEach(function (id) { element(id); });
elements["f-text"].tagName = "INPUT";
elements["f-file-search"].tagName = "INPUT";
elements["f-compose"].tagName = "TEXTAREA";
elements["view-main"].style.display = "";

var context = {
    document: doc,
    Date: Date,
    byId: function (id) { return elements[id] || null; },
    setTimeout: function (fn) { timers.push(fn); return timers.length; },
    clearTimeout: function () {},
    esc: function (text) {
        return String(text).replace(/&/g, "&amp;").replace(/</g, "&lt;");
    }
};
vm.createContext(context);
vm.runInContext(fs.readFileSync("kpm/waf/script.js", "utf8"), context);
context.sendCmd = function (op) { sent.push(op); };
context.holdVkb = function () {
    context.kbWanted = true;
    elements["f-text"].focus();
};
function flush() { while (timers.length) { timers.shift()(); } }

assert.equal(context.shellQuote("README.md"), "README.md");
assert.equal(context.shellQuote("my file.txt"), "'my file.txt'");

context.pickerRoot = "/home/proj";
context.lastPickerCwd = "/home/proj";
context.filePickerOpen = true;
doc.activeElement = elements["f-file-search"];
sent = [];
context.applyFileRow({ name: "src", dir: true });
assert.deepEqual(sent, [{ op: "list_files", path: "/home/proj/src" }]);
assert.equal(elements["file-picker"].style.display, "",
    "directories stay in the picker");
assert.equal(elements["term-compose"].style.display, "none");

sent = [];
context.applyFileRow({ name: "my file.txt", dir: false });
flush();
assert.equal(sent.length, 0, "a file pick must not type into the terminal");
assert.equal(elements["file-picker"].style.display, "none");
assert.equal(elements["term-compose"].style.display, "");
assert.equal(elements["f-compose"].value, "'my file.txt'");
assert.strictEqual(doc.activeElement, elements["f-compose"]);

context.filePickerOpen = true;
elements["file-picker"].style.display = "";
context.pickerRoot = "/tmp";
context.lastPickerCwd = "/tmp";
context.insertPickedFile("safe_name");
flush();
assert.equal(elements["f-compose"].value, "safe_name");
assert.equal(elements["btn-files"].className, "");

console.log("waf files: ok");
