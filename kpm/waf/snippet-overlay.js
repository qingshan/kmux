/* Quick snippets overlay. Kept as a classic ES5 script because Mesquite's
 * WebKit has no module support; it shares the WAF's existing global helpers. */

/* Packaged snippets are independent from runtime clipboard history. Invalid
 * records are ignored so a hand-edited JSON file cannot break the terminal. */
function parseQuickSnippets(data) {
    var raw = (data && data.snippets) || [];
    var items = [];
    var i;
    for (i = 0; i < raw.length; i++) {
        if (raw[i] && typeof raw[i].label === "string" &&
                typeof raw[i].text === "string" && raw[i].label && raw[i].text) {
            items.push({ label: raw[i].label, text: raw[i].text });
        }
    }
    return items;
}

function hideSnippetPop() {
    var pop = byId("snippet-pop");
    if (pop) {
        pop.style.display = "none";
    }
}

function showSnippetPop() {
    prepareOverlay("snippet-pop");
    var pop = byId("snippet-pop");
    var list = byId("snippet-list");
    var html = "";
    var i;
    if (!pop || !list) {
        return;
    }
    if (!quickSnippets.length) {
        list.innerHTML = '<div class="paste-empty">No quick snippets packaged</div>';
    } else {
        for (i = 0; i < quickSnippets.length; i++) {
            html += '<div class="paste-item" data-snippet-index="' + i + '">' +
                '<b>' + esc(quickSnippets[i].label) + '</b>: ' +
                esc(pastePreview(quickSnippets[i].text)) + '</div>';
        }
        list.innerHTML = html;
    }
    pop.style.display = "";
}

function loadQuickSnippet(index) {
    var item = quickSnippets[index];
    hideSnippetPop();
    if (!item) {
        return;
    }
    showCompose(item.text);
}

function fetchQuickSnippets() {
    var xhr = new XMLHttpRequest();
    try {
        xhr.open("GET", "quick-snippets.json?t=" + new Date().getTime(), true);
    } catch (err) {
        return;
    }
    xhr.onreadystatechange = function () {
        if (xhr.readyState !== 4) {
            return;
        }
        quickSnippets = [];
        if (xhr.status === 200 || xhr.status === 0) {
            try {
                quickSnippets = parseQuickSnippets(JSON.parse(xhr.responseText));
            } catch (e) {}
        }
    };
    xhr.send(null);
}
