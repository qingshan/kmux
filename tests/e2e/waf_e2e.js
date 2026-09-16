/* The WAF portion of the single E2E suite.  Keep each feature scenario in its
 * focused module, but expose one executable test entry to CI and developers. */
[
    "waf_snippets.js",
    "waf_unified.js",
    "waf_controls.js",
    "waf_agents.js",
    "waf_overlays.js",
    "waf_menu_groups.js",
    "waf_setup.js"
].forEach(function (scenario) { require("./" + scenario); });
console.log("WAF E2E: all feature scenarios OK");
