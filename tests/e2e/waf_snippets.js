/* Minimal ES5 WAF logic test. The DOMContentLoaded callback is deliberately
 * not run; this exposes the JSON parser without requiring a modern browser. */
var assert = require("assert");
var fs = require("fs");
var vm = require("vm");

var source = fs.readFileSync("kpm/waf/script.js", "utf8") + "\n" +
    fs.readFileSync("kpm/waf/snippet-overlay.js", "utf8");
var context = {
    document: { addEventListener: function () {} },
    setTimeout: function () { return 0; },
    clearTimeout: function () {},
    Date: Date
};
vm.createContext(context);
vm.runInContext(source, context);

function parsed(value) {
    return JSON.parse(JSON.stringify(context.parseQuickSnippets(value)));
}

var catalog = JSON.parse(fs.readFileSync("kpm/waf/quick-snippets.json", "utf8"));
var packaged = parsed(catalog);
assert(packaged.length > 0, "packaged snippets must not be empty");
packaged.forEach(function (item) {
    assert.strictEqual(typeof item.label, "string");
    assert.strictEqual(typeof item.text, "string");
    assert(item.label.length > 0);
    assert(item.text.length > 0);
});
assert(packaged.some(function (item) { return item.text.indexOf("git ") === 0; }),
    "catalog must include shell commands");
assert(packaged.some(function (item) { return /continue|review|explain/i.test(item.text); }),
    "catalog must include agent prompts");
assert.deepStrictEqual(
    parsed({ snippets: [
        { label: "Deploy", text: "just package" },
        { label: "", text: "ignored" },
        { label: "missing" },
        null
    ] }),
    [{ label: "Deploy", text: "just package" }]
);
console.log("waf snippets: ok");
