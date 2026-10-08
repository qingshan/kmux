/* Mesquite exposes appmgr.ongo with a setter but no native getter. */
var assert = require('assert');
var fs = require('fs');
var vm = require('vm');
var ready;
var ongo;
var appmgr = {};
Object.defineProperty(appmgr, 'ongo', {
    get: function () { throw new ReferenceError('write-only native callback'); },
    set: function (fn) { ongo = fn; }
});
var context = {
    window: {kindle: {appmgr: appmgr}},
    document: {addEventListener: function (name, fn) {
        if (name === 'DOMContentLoaded') { ready = fn; }
    }}
};
vm.createContext(context);
vm.runInContext(fs.readFileSync('common/waf-base.js', 'utf8'), context);
vm.runInContext(fs.readFileSync('kpm/waf/script.js', 'utf8'), context);
var painted = {};
context.paintFullGrid = function () { throw painted; };
assert.throws(function () { ready(); }, function (error) { return error === painted; },
    'startup reaches the first grid paint without reading the native callback');
var calls = [];
context.updateChrome = function (app, title) { calls.push([app, title]); };
context.sendCmd = function (op) { calls.push(op.op); };
context.fetchStatusJson = function () { calls.push('status'); };
context.startAutoRefresh = function () { calls.push('refresh'); };
ongo();
assert.deepEqual(calls, [['dev.qingshan.kmux', 'kmux'], 'watch_start', 'status', 'refresh'],
    'reopening restores chrome, the daemon watch, and status polling');
console.log('WAF startup: write-only Mesquite callback and reopen OK');
