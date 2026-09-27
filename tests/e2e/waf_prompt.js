/* Agent Prompt replies are explicit taps: y/n/1/2/3/Continue send text+Enter. */
var assert = require("assert");
var fs = require("fs");
var vm = require("vm");

var catalog = JSON.parse(fs.readFileSync("kpm/waf/prompt-rules.json", "utf8"));
var ai = catalog.rules.filter(function (r) { return r.id === "ai"; })[0];
assert(ai);
var labels = ai.keys.map(function (k) { return k.label; });
["y", "n", "1", "2", "3", "Continue"].forEach(function (label) {
    var key = ai.keys.filter(function (k) { return k.label === label; })[0];
    assert(key && key.prompt === (label === "Continue" ? "Continue" : label),
        label + " must send that reply plus Enter");
});
assert(labels.indexOf("y") < labels.indexOf("/clear"),
    "replies stay ahead of slash commands");

var elements = { "prompt-list": { innerHTML: "" } };
var context = {
    document: { addEventListener: function () {} },
    setTimeout: function () { return 1; },
    clearTimeout: function () {},
    Date: Date,
    byId: function (id) { return elements[id] || null; },
    esc: function (s) {
        return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
    }
};
vm.createContext(context);
vm.runInContext(fs.readFileSync("kpm/waf/script.js", "utf8"), context);
context.promptRulesFile = catalog;

assert.equal(context.pickPromptRule({ cmd: "codex" }).id, "ai");
assert.equal(context.pickPromptRule({ cmd: "node", agent: "claude" }).id, "ai",
    "detected agent name selects AI replies even inside node");
assert.equal(context.pickPromptRule({ cmd: "fish", agent: "" }).id, "shell");
assert.equal(context.pickPromptRule({ cmd: "python3" }).id, "python");

var html = context.promptButtonHtml({ label: "y", prompt: "y", title: "Yes" });
assert(html.indexOf('data-prompt="y"') >= 0);
assert(html.indexOf("data-key") < 0);

context.lastData = {
    session: "s1",
    activeHost: "devbox",
    windows: ["W s1 t1 Shell *"],
    panes: ["P s1 t1 %3 0 node [80x24] 1"],
    agentPanes: [{ id: "%3", agent: { name: "grok", state: "blocked" } }]
};
assert.equal(context.currentPaneInfo(context.lastData).agent, "grok");
context.renderPromptPanel();
assert(elements["prompt-list"].innerHTML.indexOf('data-prompt="y"') >= 0);
assert(elements["prompt-list"].innerHTML.indexOf('data-prompt="Continue"') >= 0);
assert(elements["prompt-list"].innerHTML.indexOf(">y<") >= 0);

assert(/#panel-prompt/.test(fs.readFileSync("kpm/waf/style.css", "utf8")));
console.log("waf prompt: ok");
