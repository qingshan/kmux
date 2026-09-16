/*
 * KMux WAF — terminal view of a remote tmux session.
 *
 * ES5 only (Mesquite's old WebKit). kmuxd parses the tmux control-mode
 * stream into a screen model and writes it to status.json (screen rows +
 * parallel attrs, cursor, scrollback). This page polls status.json every
 * 2.5s, and also right after a key/text so typing does not wait for the
 * next interval. The 80x24 table is patched in place (only dirty rows)
 * so e-ink does not full-refresh the pane on every character. Keys/text
 * go out as control commands (watch_start on open, key/text ops on tap).
 * Scrollback paging is client-side over the screen + scrollback rows.
 */

var KMUX_ID = "dev.qingshan.kmuxd";
var KMUX_APP = "dev.qingshan.kmux";

/* Nerd Font icon codepoints (glyphs verified present in the bundled font). */
var ICON_SESSION = "\uF120"; /* session/workspace: fa-terminal */
var ICON_ACTIVE = "\uF0DA"; /* current window marker: caret-right */
var ICON_SERVER = "\uF233"; /* host: fa-server */
var ICON_PANE = "\uF0DB"; /* pane: fa-columns */
var ICON_AGENT = "\uF007"; /* agent: fa-user */
var ICON_MORE = "\uF141"; /* more-actions: fa-ellipsis-h */
var ICON_FOLDER = "\uF07B"; /* file picker: fa-folder */
var ICON_FILE = "\uF15B"; /* file picker: fa-file */

function menuKindIcon(kind) {
    if (kind === "host") { return ICON_SERVER; }
    if (kind === "session") { return ICON_SESSION; }
    if (kind === "pane") { return ICON_PANE; }
    if (kind === "agent") { return ICON_AGENT; }
    return ICON_SESSION;
}

var refreshTimer = null;
var lastData = null;
var lastClipFromDaemon = ""; /* last status.clipboard we pushed to GTK */
var viewOffset = 0; /* rows back into the scrollback (0 = live screen) */
var searchNeedle = ""; /* lowercase; empty = no highlight */
var cmdHistIdx = -1; /* hist index of selected shell command, or -1 */
var lastDaemonOffset = -1; /* offset we last sent / acked; stale polls
                              with a different value are ignored */
var scrollAckUntil = 0; /* ignore daemon offset until this time, or ack */
var lastSbLen = -1; /* previous scrollback length; grow viewOffset with it */
var paneOffsets = {}; /* paneKey -> { off, sb } so switch/create/close
                         restores a pane that was in scrollback */
var lastPaneKey = "";
var pendingRestore = null; /* saved offset while recapture sb is empty */
var PRESS_FLASH_MS = 300; /* e-ink needs a held invert before navigation */
var switchPending = false; /* window/pane op in flight: paint Live until
                              the destination pane id is known */
var awaitHost = ""; /* host id we switched to; hold paint until connected */
var dragActive = false; /* a terminal drag is in progress (suppresses the
                           keyboard click + the poll's offset sync) */
var dragEndAt = 0;
var kbTimer = null;
var dragMoved = false; /* the current touch has moved >=2px - not a tap */ /* mousedown-only taps: the WebKit never delivers
                       mouseup/click, so the keyboard is opened from the
                       term's mousedown after a short delay that a real
                       drag cancels */ /* the click after a drag is suppressed only until this
                      deadline, so a lost mouseup can't block the keyboard */
var renderTimer = null; /* e-ink can't repaint per-mousemove: throttle */
var lastPaintedRows = []; /* previous <tr> HTML, for in-place row patches */
var lastPaintedCols = 0;
var lastWinSig = ""; /* skip rewriting the window-tab strip when unchanged */
var lastInputAt = 0; /* last key/text: suppress the periodic list-windows */
var expectChangeFrom = null; /* visibleSig before an input; ignore stale polls */
var expectChangeUntil = 0;
var pokeSeq = 0; /* incrementing id so a newer key cancels older pokes */
var lastStateText = "";
var lastStateClass = "";
var lastStateHidden = false; /* plain "Live": badge overlay is hidden */
var lastErrorText = null;
var lastTermTapAt = 0;
var composeSending = false;
var composeGeneration = 0;
var lastScrollRevision = 0;
var setupDialogShown = false;
var settingsDirty = false;

function queueRender() {
    if (renderTimer) {
        return;
    }
    renderTimer = setTimeout(function () {
        renderTimer = null;
        if (lastData) {
            safeRender(lastData);
        }
    }, 250);
}

var overlayIds = ["win-menu", "act-menu", "file-picker", "paste-pop",
    "snippet-pop", "term-compose", "view-settings", "err-pop"];

function overlayVisible(id) {
    var el = byId(id);
    return !!(el && el.style.display !== "none");
}

function hideSettingsOverlay() {
    var el = byId("view-settings");
    var focused = document.activeElement;
    var node = focused;
    while (node && node !== document) {
        if (node === el) { if (focused.blur) { focused.blur(); } break; }
        node = node.parentNode;
    }
    if (el) { el.style.display = "none"; }
    var gear = byId("tab-settings");
    if (gear) { gear.className = "tab"; }
    currentView = "view";
}

function closeOverlay(id) {
    if (id === "term-compose") { cancelCompose(); }
    else if (id === "win-menu") { closeWinMenu(); }
    else if (id === "act-menu") { closeActMenu(); }
    else if (id === "file-picker") { closeFilePicker(); }
    else if (id === "paste-pop") { hidePastePop(); }
    else if (id === "snippet-pop") { hideSnippetPop(); }
    else if (id === "view-settings") { hideSettingsOverlay(); }
    else if (id === "err-pop") { hideErrPop(); }
}

function prepareOverlay(id) {
    for (var i = 0; i < overlayIds.length; i++) {
        if (overlayIds[i] !== id && overlayVisible(overlayIds[i])) {
            closeOverlay(overlayIds[i]);
        }
    }
}

function dismissVisibleOverlay() {
    for (var i = 0; i < overlayIds.length; i++) {
        if (overlayVisible(overlayIds[i])) {
            closeOverlay(overlayIds[i]);
            keepCaptureFocus();
            return true;
        }
    }
    return false;
}

function wireOverlayChrome() {
    for (var i = 0; i < overlayIds.length; i++) {
        (function (id) {
            var button = byId("close-" + id);
            if (!button) { return; } // Compose owns its cancellation handler.
            button.addEventListener("mousedown", function (ev) {
                if (ev.preventDefault) { ev.preventDefault(); }
                if (ev.stopPropagation) { ev.stopPropagation(); }
                closeOverlay(id);
                keepCaptureFocus();
            });
        }(overlayIds[i]));
    }
    document.addEventListener("keydown", function (ev) {
        if ((ev.keyCode || ev.which) === 27 && dismissVisibleOverlay()) {
            if (ev.preventDefault) { ev.preventDefault(); }
            if (ev.stopPropagation) { ev.stopPropagation(); }
        }
    });
}

function setError(text) {
    text = text || "";
    if (text === lastErrorText) {
        return;
    }
    lastErrorText = text;
    /* Surface errors the moment they arrive; a cleared error hides the
       popup. (Same text is deduped above, so an unchanged Offline error
       does not re-pop on every poll.) */
    var pop = byId("err-pop");
    if (!pop) {
        return;
    }
    if (text) {
        byId("error-message").innerHTML = esc(text);
        // Do not cover an editor or another popup when a background poll fails.
        for (var i = 0; i < overlayIds.length; i++) {
            if (overlayIds[i] !== "err-pop" && overlayVisible(overlayIds[i])) { return; }
        }
        pop.style.display = "";
    } else {
        pop.style.display = "none";
    }
}

function syncSettings(data) {
    if (settingsDirty || !data) { return; }
    var proxy = byId("f-proxy");
    var socks = byId("f-socks5");
    if (proxy && typeof data.proxyUrl === "string") { proxy.value = data.proxyUrl; }
    if (socks && typeof data.socks5 === "string") { socks.value = data.socks5; }
}

function backendLabel(backend) {
    return backend === "herdr" ? "Herdr" : "tmux";
}

function diagnoseLine(ok, name, detail) {
    var text = (ok ? "ok" : "fail") + "  " + name;
    if (detail) { text += "  " + detail; }
    return '<div' + (ok ? "" : ' class="fail"') + '>' + esc(text) + "</div>";
}

function paintDiagnose(report) {
    var box = byId("diagnose-report");
    if (!box || !report) { return; }
    var html;
    if (report.running) {
        html = "<div>Checking&hellip;</div>";
    } else {
        var proxy = report.proxy || {};
        html = diagnoseLine(!!proxy.ok, "proxy", proxy.detail || "");
        var hosts = report.hosts || [];
        var i;
        for (i = 0; i < hosts.length; i++) {
            var host = hosts[i];
            var ssh = host.ssh || {};
            var service = host.service || {};
            var detail;
            if (!host.ok) {
                detail = (!ssh.ok ? ssh.detail : service.detail) || "";
            } else {
                detail = backendLabel(host.backend);
                if (service.detail) { detail += " " + service.detail; }
            }
            html += diagnoseLine(!!host.ok, host.name || host.id, detail);
        }
        if (!hosts.length && proxy.ok) {
            html += "<div>No machines configured</div>";
        }
    }
    box.innerHTML = html;
    box.style.display = "";
}

function showSetupSettings() {
    if (setupDialogShown) { return; }
    setupDialogShown = true;
    showView("settings");
}

/* Error popup under the status chip: toggled by tapping the chip, closed
   by tapping anywhere else (or by opening another overlay). */
function toggleErrPop() {
    var pop = byId("err-pop");
    if (!pop) {
        return;
    }
    if (pop.style.display !== "none") {
        pop.style.display = "none";
        return;
    }
    if (lastErrorText) {
        prepareOverlay("err-pop");
        byId("error-message").innerHTML = esc(lastErrorText);
        pop.style.display = "";
    }
}

function hideErrPop() {
    var pop = byId("err-pop");
    if (pop) {
        pop.style.display = "none";
    }
}

function hidePastePop() {
    var pop = byId("paste-pop");
    if (pop) {
        pop.style.display = "none";
    }
}

function pastePreview(text) {
    var s = String(text || "").replace(/\s+/g, " ");
    if (s.length > 80) {
        s = s.substring(0, 80) + "\u2026";
    }
    return s;
}

function showPastePop() {
    prepareOverlay("paste-pop");
    var pop = byId("paste-pop");
    var list = byId("paste-list");
    var items = (lastData && lastData.clipboardHistory) || [];
    var i;
    var html = "";
    if (!pop || !list) {
        return;
    }
    if (!items.length) {
        list.innerHTML = '<div class="paste-empty">Clipboard history is empty</div>';
    } else {
        for (i = 0; i < items.length; i++) {
            html += '<div class="paste-item" data-clip-index="' + i + '">' +
                esc(pastePreview(items[i])) + '</div>';
        }
        list.innerHTML = html;
    }
    pop.style.display = "";
}

function loadClipboardHistory(index) {
    var items = (lastData && lastData.clipboardHistory) || [];
    var text = items[index];
    hidePastePop();
    if (typeof text !== "string") {
        return;
    }
    showCompose(text);
}

function sendCmd(op) {
    if (op && (op.op === "text" || op.op === "key" || op.op === "copy_mode" || op.op === "copy_search")) {
        var pane = currentPaneInfo(lastData);
        op.expected_pane = pane.id || null;
        op.expected_machine = (lastData && lastData.activeHost) || "";
    }
    var ok = sendJsonCmd(KMUX_ID, op);
    if (ok && op) {
        var kind = op.op;
        if (kind === "key" || kind === "text") {
            lastInputAt = Date.now();
            /* Capture the pre-input screen once per burst so a stale
               status.json (tmux has not echoed yet) cannot flash the old
               frame over the first character. Window/pane switches must
               not use this gate — it skipped tab redraws for 2s and
               looked like copy mode had locked the session. */
            if (expectChangeFrom === null) {
                expectChangeFrom = visibleSig(lastData);
            }
            expectChangeUntil = Date.now() + 900;
            pokeScreen();
        } else if (kind === "window" || kind === "pane" ||
                kind === "new_window" || kind === "next_window" ||
                kind === "prev_window" || kind === "kill_window" ||
                kind === "detach" || kind === "session" ||
                kind === "new_session" || kind === "host" || kind === "host_session" || kind === "host_pane") {
            lastInputAt = Date.now();
            expectChangeFrom = null;
            pokeScreen();
        } else if (kind === "list_files" || kind === "copy_mode" ||
                kind === "watch_start") {
            /* Refresh status.json for the picker / WAF reopen; do not
               treat this as typed input waiting for a terminal echo. */
            pokeScreen();
        }
    }
    return ok;
}

function fetchStatusJson() {
    pollStatusJson(safeRender, "status.json missing - is kmuxd running? Tap the Home screen icon to reopen.");
}

/* Cursor + live rows + scroll offset: enough to tell "tmux has not
   echoed this key yet" from "the screen moved". */
function visibleSig(data) {
    if (!data) {
        return "";
    }
    return String(data.cursorX || 0) + "," + String(data.cursorY || 0) + "," +
        String(data.scrollOffset || 0) + "," +
        (data.connected ? "1" : "0") + "," +
        (data.copyMode ? "C" : "L") + "," +
        (data.windows ? data.windows.join("|") : "") + "," +
        (data.screen ? data.screen.join("\n") : "");
}

/* After a key, poll a few times across the SOCKS5 round-trip instead of
   waiting up to 2.5s for the background interval. */
function pokeScreen() {
    pokeSeq += 1;
    var seq = pokeSeq;
    function later(ms) {
        setTimeout(function () {
            if (seq === pokeSeq) {
                fetchStatusJson();
            }
        }, ms);
    }
    later(70);
    later(180);
    later(350);
    later(600);
    /* new-window: the shell prompt often lands after the first capture
       (which is blank) — keep polling until it shows. */
    later(1000);
    later(1800);
}

/* Patch dirty <tr>s in place. A full innerHTML of 80x24 cells forces the
   e-ink driver to refresh the whole pane; one row is a small rectangle.
   Fall back to a full rebuild when most rows changed (page, redraw) or
   if this WebKit rejects tr.innerHTML. */
function paintScreen(htmlRows, cols) {
    var wrap = byId("term");
    var table = wrap.getElementsByTagName("table")[0];
    var i;
    var dirty = 0;
    if (table && lastPaintedCols === cols && table.rows.length === htmlRows.length) {
        for (i = 0; i < htmlRows.length; i++) {
            if (lastPaintedRows[i] !== htmlRows[i]) {
                dirty += 1;
            }
        }
        if (dirty === 0) {
            return;
        }
        if (dirty <= htmlRows.length / 2) {
            try {
                for (i = 0; i < htmlRows.length; i++) {
                    if (lastPaintedRows[i] !== htmlRows[i]) {
                        var inner = htmlRows[i];
                        if (inner.substring(0, 4) === "<tr>") {
                            inner = inner.substring(4, inner.length - 5);
                        }
                        table.rows[i].innerHTML = inner;
                        lastPaintedRows[i] = htmlRows[i];
                    }
                }
                return;
            } catch (err) {
                /* fall through to a full table rebuild */
            }
        }
    }
    var html = "<colgroup>";
    for (i = 0; i < cols; i++) {
        html += "<col>";
    }
    html += "</colgroup>" + htmlRows.join("");
    wrap.innerHTML = "<table class=\"term\" cellpadding=\"0\" cellspacing=\"0\">" +
        html + "</table>";
    lastPaintedCols = cols;
    lastPaintedRows = htmlRows.slice(0);
}

/* kmuxd emits one Unicode scalar per cell. Mesquite has no codePointAt,
   so walk UTF-16 without splitting surrogate pairs (e.g. 🌐). */
function scalars(s) {
    var out = [];
    var i = 0;
    while (i < s.length) {
        var c = s.charCodeAt(i);
        if (c >= 0xD800 && c <= 0xDBFF && i + 1 < s.length) {
            var d = s.charCodeAt(i + 1);
            if (d >= 0xDC00 && d <= 0xDFFF) {
                out.push(s.substring(i, i + 2));
                i += 2;
                continue;
            }
        }
        out.push(s.charAt(i));
        i += 1;
    }
    return out;
}

function termTd(cls, text, span) {
    var inner = esc(text);
    if (!inner || inner === " ") {
        inner = "\u00a0";
    }
    return "<td" + (span > 1 ? " colspan=\"" + span + "\"" : "") +
        (cls ? " class=\"" + cls + "\"" : "") + ">" + inner + "</td>";
}

function attrClass(ch) {
    if (ch === "b") { return "b"; }
    if (ch === "i") { return "i"; }
    if (ch === "B") { return "bi"; }
    return "";
}

/* Display columns for one scalar. kmuxd stores one cell per Unicode
   scalar; i still advances by 1 after a wide glyph (the extra column
   comes from the table, not by eating the next cell).
   - Emoji / CJK: 2. A 1-col <td> with overflow:visible painted the
     globe over the space in "🌐 devbox".
   - Nerd Font PUA: 1. U+F054 is one tmux column; colspan=2 put the
     prompt cursor one cell right of tmux. */
function glyphCols(ch) {
    if (!ch) {
        return 1;
    }
    var c0 = ch.charCodeAt(0);
    if (c0 >= 0xD800 && c0 <= 0xDBFF) {
        return 2;
    }
    if (c0 >= 0xE000 && c0 <= 0xF8FF) {
        return 1;
    }
    if ((c0 >= 0x1100 && c0 <= 0x115F) ||
            (c0 >= 0x2E80 && c0 <= 0xA4CF && c0 !== 0x303F) ||
            (c0 >= 0xAC00 && c0 <= 0xD7A3) ||
            (c0 >= 0xF900 && c0 <= 0xFAFF) ||
            (c0 >= 0xFE10 && c0 <= 0xFE19) ||
            (c0 >= 0xFE30 && c0 <= 0xFE6F) ||
            (c0 >= 0xFF00 && c0 <= 0xFF60) ||
            (c0 >= 0xFFE0 && c0 <= 0xFFE6)) {
        return 2;
    }
    return 1;
}

/* Wide glyphs get colspan=2; extra width is taken from trailing
   columns so the row still fills `cols`. */
function rowHtml(text, attrs, rowIdx, histIdx) {
    var cols = (lastData && lastData.cols) ? lastData.cols : 80;
    var cells = scalars(text);
    while (cells.length < cols) {
        cells.push(" ");
    }
    if (cells.length > cols) {
        cells.length = cols;
    }
    var a = attrs || "";
    while (a.length < cols) {
        a += ".";
    }
    a = a.substring(0, cols);

    var cursorAt = null;
    if (rowIdx !== null && lastData && rowIdx === lastData.cursorY && viewOffset === 0) {
        cursorAt = lastData.cursorX;
        if (cursorAt < 0 || cursorAt >= cols) {
            cursorAt = null;
        }
    }
    var hits = hitCells(text, searchNeedle);
    if (histIdx === cmdHistIdx) {
        var cmdHits = promptCmdHits(text);
        var hk;
        if (cmdHits) {
            for (hk in cmdHits) {
                if (cmdHits.hasOwnProperty(hk)) {
                    hits[hk] = true;
                }
            }
        }
    }

    var tds = [];
    var i = 0;
    var used = 0;
    while (i < cols && used < cols) {
        var w = glyphCols(cells[i]);
        if (w < 1) {
            w = 1;
        }
        if (used + w > cols) {
            w = cols - used;
        }
        var cls = (cursorAt !== null && i === cursorAt) ? "cur" : attrClass(a.charAt(i));
        if (hits[i]) {
            cls = cls ? (cls + " hl") : "hl";
        }
        tds.push(termTd(cls, cells[i], w));
        used += w;
        i += 1;
    }
    while (used < cols) {
        tds.push(termTd("", "\u00a0", 1));
        used += 1;
    }
    return "<tr>" + tds.join("") + "</tr>";
}

function fillTermHtmlRows(htmlRows, nrows) {
    htmlRows = htmlRows || [];
    nrows = nrows || 24;
    while (htmlRows.length < nrows) {
        var pad = htmlRows.length;
        htmlRows.push(rowHtml("", "", pad, pad));
    }
    if (htmlRows.length > nrows) {
        htmlRows.length = nrows;
    }
    return htmlRows;
}

function paintFullGrid(htmlRows, cols, nrows) {
    paintScreen(fillTermHtmlRows(htmlRows, nrows || 24), cols || 80);
}

function safeRender(data) {
    try {
        render(data);
    } catch (err) {
        /* A render failure must not silently blank the terminal — surface
           it so the state chip shows what went wrong. */
        setError("render error: " + (err && err.message ? err.message : err));
    }
}

function render(data) {
    /* A poke after a key can land on a snapshot written before tmux
       echoed. Hold only the terminal grid so a stale frame does not
       flash; tabs/copyMode still update (a window switch used to look
       like copy mode had locked the session). */
    var holdTerm = false;
    if (expectChangeFrom !== null && Date.now() < expectChangeUntil) {
        if (visibleSig(data) === expectChangeFrom) {
            holdTerm = true;
        } else {
            expectChangeFrom = null;
        }
    } else {
        expectChangeFrom = null;
    }

    var leavingCopy = !!(lastData && lastData.copyMode && !data.copyMode);
    if (data.clipboard && data.clipboard !== lastClipFromDaemon) {
        lastClipFromDaemon = data.clipboard;
        copyToKindleClipboard(data.clipboard);
    }
    lastData = data;
    syncSettings(data);
    paintDiagnose(data.diagnose);
    if (!data.configured) {
        setState("Setup needed", "state-error");
        setError(data.lastError || "Configure the proxy URL and token in Settings.");
        paintFullGrid([], data.cols || 80, data.rows || 24);
        showSetupSettings();
        return;
    }
    setError(data.lastError || "");

    var rows = data.screen || [];
    /* Always paint a full terminal frame: with no screen rows yet (e.g.
       Offline before the first capture) the terminal would otherwise
       collapse to an empty box. */
    if (!rows.length) {
        var blankRows = data.rows || 24;
        rows = [];
        for (var bi = 0; bi < blankRows; bi++) {
            rows.push("");
        }
    }
    var attrs = data.attrs || [];
    var sb = data.scrollback || [];
    var haveContent = screenHasContent(rows, sb);

    /* Each pane keeps its own scrollback until Live. Switch/create/close
       wipes the model (empty sb) then recaptures; save/restore so the
       pane you left does not slide toward Live, and a new pane starts
       live. */
    if (lastPaneKey && lastSbLen > 0 && !pendingRestore && !switchPending &&
            !data.copyMode) {
        paneOffsets[lastPaneKey] = { off: viewOffset, sb: lastSbLen };
    }
    var key = paneKey(data);
    if (key && key !== lastPaneKey) {
        /* Pane-id keys are stable; window-index keys are a fallback until
           list-panes lands. Carry the offset across that rename only on
           the same host (not across a host switch). */
        var sameHost = key.indexOf("/") >= 0 && lastPaneKey.indexOf("/") >= 0 &&
            key.split("/")[0] === lastPaneKey.split("/")[0];
        if (!paneOffsets[key] && lastPaneKey &&
                lastPaneKey.indexOf(":w:") !== -1 && sameHost &&
                paneOffsets[lastPaneKey]) {
            paneOffsets[key] = paneOffsets[lastPaneKey];
        }
        lastPaneKey = key;
        pendingRestore = paneOffsets[key] || { off: 0, sb: -1 };
        switchPending = false;
        cmdHistIdx = -1;
    } else if (switchPending && key && key === lastPaneKey) {
        /* Pane list has not caught up: this is still the source key on a
           new screen. Do not restore the source offset onto it. */
        pendingRestore = { off: 0, sb: -1 };
        if (haveContent) {
            switchPending = false;
        }
    }
    if (switchPending) {
        viewOffset = 0;
    }
    var hostHold = false;
    if (awaitHost) {
        if (data.activeHost !== awaitHost || !data.connected || !haveContent) {
            hostHold = true;
        } else {
            awaitHost = "";
            lastPaintedRows = [];
            lastPaintedCols = 0;
        }
    }
    if (pendingRestore) {
        if (haveContent) {
            viewOffset = pendingRestore.off || 0;
            if (viewOffset > 0 && pendingRestore.sb >= 0 &&
                    sb.length > pendingRestore.sb) {
                viewOffset += sb.length - pendingRestore.sb;
            }
            lastSbLen = sb.length;
            lastDaemonOffset = viewOffset;
            scrollAckUntil = Date.now() + 1500;
            sendCmd({ op: "scroll_to", offset: viewOffset });
            pendingRestore = null;
        }
    } else if (sb.length === 0 && viewOffset > 0 && lastSbLen > 0) {
        /* In-transit wipe: keep the offset until the recapture lands. */
    } else if (sb.length < lastSbLen) {
        if (viewOffset === 0) {
            lastSbLen = sb.length;
        }
    } else {
        if (viewOffset > 0 && lastSbLen > 0 && sb.length > lastSbLen) {
            viewOffset += sb.length - lastSbLen;
            lastDaemonOffset = viewOffset;
        }
        lastSbLen = sb.length;
    }

    syncDaemonScroll(data);
    /* Empty sb after a wipe is in-transit: do not clamp to Live. */
    if (sb.length > 0 && viewOffset > sb.length) {
        viewOffset = sb.length;
    }
    /* Copy mode paints capture-pane -M in `screen`, not local scrollback. */
    if (data.copyMode) {
        viewOffset = 0;
    }
    /* The chip reports the same (clamped) offset the terminal renders. A
       plain "Live" (connected, live screen, no alt screen) needs no badge,
       so the overlay is hidden then and only appears for Offline /
       Connecting… / Alt / scrolled-back states. */
    var st = data.connected
        ? (data.copyMode ? "Copy" : (data.altScreen ? "Alt" : "Live"))
        : "Offline";
    if (viewOffset > 0) {
        st += " \u00b7 " + viewOffset + " back";
    }
    var stClass = data.connected ? "state-ok" : "state-error";
    var stHidden = st === "Live";
    if (st !== lastStateText || stClass !== lastStateClass ||
            stHidden !== lastStateHidden) {
        lastStateText = st;
        lastStateClass = stClass;
        lastStateHidden = stHidden;
        var stateEl = byId("state");
        if (stHidden) {
            if (stateEl) {
                stateEl.style.display = "none";
            }
        } else {
            setState(st, stClass);
            if (stateEl) {
                stateEl.style.display = "";
            }
        }
    }

    if (!data.connected || data.altScreen) {
        clearMods();
    }

    var hostSig = "";
    var hs = data.hosts || [];
    var hi;
    for (hi = 0; hi < hs.length; hi++) {
        hostSig += (hs[hi] && hs[hi].id ? hs[hi].id : "") + ":" +
            (hs[hi] && hs[hi].name ? hs[hi].name : "") + ",";
    }
    var winSig = (data.activeHost || "") + "\n" + (data.session || "") + "\n" +
        hostSig + "\n" + (data.windows || []).join("\n") + "\n" + JSON.stringify(data.tabAgents || {});
    if (winSig !== lastWinSig) {
        lastWinSig = winSig;
        renderTabs(data);
    }
    var pinfo = currentPaneInfo(data);
    var promptSig = (pinfo.cmd || "") + "\n" + (pinfo.win || "");
    if (promptSig !== lastPromptSig) {
        lastPromptSig = promptSig;
        cmdHistIdx = -1;
        renderPromptPanel();
    }

    var cols = data.cols || 80;
    var htmlRows = [];
    var i;
    /* Keep the last 80x24 while switch/create/close has wiped the model
       and the destination capture has not arrived. Painting that empty
       snapshot collapsed the table or flashed a blank pane. */
    if (leavingCopy && haveContent) {
        lastPaintedRows = [];
    }
    /* Always hold the current grid when the new snapshot has no cells
       (pane switch, connect). First paint (no rows yet) still fills a
       blank 80x24 so Connecting is not a border-only box. */
    if (lastPaintedRows.length && (holdTerm || hostHold || !haveContent)) {
        if (menuOpen) {
            renderWindowMenu(data);
        }
        if (filePickerOpen) {
            renderFilePicker(data);
        }
        return;
    }
    /* A wipe leaves viewOffset > sb.length; painting that window yields
       zero <tr>s and the terminal collapses until recapture. Clamp only
       for display so the 80x24 frame stays up. */
    var paintOff = viewOffset;
    if (paintOff > sb.length) {
        paintOff = sb.length;
    }
    if (paintOff === 0) {
        /* Live view: do not copy/reverse the 500-line scrollback. */
        for (i = 0; i < rows.length; i++) {
            var liveText = rows[i] || "";
            if (liveText.indexOf("KMUXCURSOR") !== -1) {
                liveText = "";
            }
            htmlRows.push(rowHtml(liveText, attrs[i] || "", i, sb.length + i));
        }
    } else {
        /* History in display order: scrolled-out lines (oldest first) then
           the live screen. Show EXACTLY `rows` rows — a window into the
           history — so the terminal box never grows when paging. */
        var total = sb.length + rows.length;
        var hist = sb.slice().reverse().concat(rows);
        var endIdx = total - paintOff;
        var startIdx = Math.max(0, endIdx - rows.length);
        var screenStart = Math.max(startIdx, total - rows.length);
        for (i = startIdx; i < endIdx; i++) {
            var isScreen = i >= screenStart;
            var idx = isScreen ? (i - (total - rows.length)) : null;
            var a = isScreen ? (attrs[idx] || "") : "";
            /* The daemon's cursor query ("KMUXCURSOR x y") can leak into the
               screen during a window close race — never render it. */
            var rowText = hist[i] || "";
            if (rowText.indexOf("KMUXCURSOR") !== -1) {
                rowText = "";
            }
            /* Scrollback rows are plain text — rowHtml needs a matching-length
               attrs string (all normal cells) or it renders nothing. */
            if (!isScreen && a.length === 0 && rowText) {
                a = new Array(rowText.length + 1).join(".");
            }
            htmlRows.push(rowHtml(rowText, a, idx, i));
        }
    }
    paintFullGrid(htmlRows, cols, rows.length || 24);

    if (menuOpen) {
        renderWindowMenu(data);
    }
    if (filePickerOpen) {
        renderFilePicker(data);
    }
}

/* Terminal view: session chip (opens the window dropdown) then window
   tabs for the attached session. Line format: "W sess idx name flags
   [WxH]". */
function agentBadge(summary) {
    if (!summary || !summary.count) { return ""; }
    var labels = { blocked: "Needs input", working: "Working", done: "Done", idle: "Idle", unknown: "Unknown" };
    return ' <span class="agent-badge">[' + (labels[summary.state] || "Unknown") +
        (summary.count > 1 ? " " + summary.count : "") + "]</span>";
}

function renderTabs(data) {
    var strip = byId("window-tabs");
    if (!strip) {
        return;
    }
    var wins = data.windows || [];
    var cur = data.session || "";
    var hosts = data.hosts || [];
    var chipLabel = (data.sessionNames && data.sessionNames[cur]) || cur;
    var chipIcon = ICON_SESSION;
    if (hosts.length > 1) {
        chipLabel = hostDisplayName(data) || cur;
        chipIcon = ICON_SERVER;
    } else if (!chipLabel) {
        chipLabel = hostDisplayName(data);
        if (chipLabel) {
            chipIcon = ICON_SERVER;
        }
    }
    var html = "";
    if (chipLabel) {
        html += '<span class="wintab wintab-session' + (menuOpen ? " active" : "") +
            '" id="session-chip" data-kind="session-chip" data-id="' +
            esc(chipLabel) + '" title="Windows">' + chipIcon + " " +
            esc(chipLabel) + " \u25BE</span>";
    }
    var n = 0;
    for (var i = 0; i < wins.length; i++) {
        var parts = wins[i].split(/\s+/);
        if (parts.length < 4 || parts[0] !== "W") {
            continue;
        }
        if (cur && parts[1] !== cur) {
            continue;
        }
        var idx = parts[2];
        var name = (data.tabNames && data.tabNames[idx]) || parts[3] || "";
        var active = wins[i].indexOf("*") !== -1;
        n += 1;
        html += '<span class="wintab' + (active ? " active" : "") +
            '" data-kind="wintab" data-id="' + esc(idx) + '">' +
            esc(name) + agentBadge((data.tabAgents || {})[idx]) + "</span>";
    }
    if (n > 0) {
        html += '<span class="wintab wintab-new" data-kind="wintab-new" ' +
            'data-id="new">+</span>';
        html += '<span class="wintab wintab-new' + (actMenuOpen ? " active" : "") +
            '" id="act-more" data-kind="wintab-more" data-id="more" title="Actions">' +
            ICON_MORE + "</span>";
    }
    strip.innerHTML = html;
}

/* Window dropdown: a search box over a flat list of every window on the
   tmux server, each row "session:index name" (windows line format from
   kmuxd: "W sess idx name flags [WxH]"). Rows are fuzzy-filtered by the
   search box; tapping a row switches to that session's window and closes
   the menu. A search for a valid session name no session has yet offers
   "Create session <name>" as the last row. */
function validSessionName(name) {
    return name && /^[A-Za-z0-9_.-]+$/.test(name);
}

function fuzzyScore(q, text) {
    /* Subsequence ("fuzzy") match: every q char must appear in text in
       order. Higher score for the leading char, for chars starting a
       token (after ':'/' '/'.'/'/'), and for consecutive chars. */
    var ti = 0;
    var prev = -2;
    var total = 0;
    var run = 0;
    var first = true;
    for (var qi = 0; qi < q.length; qi++) {
        var c = q.charAt(qi);
        var found = text.indexOf(c, ti);
        if (found < 0) {
            return 0;
        }
        var step = 1;
        if (first && found === 0) {
            step += 8;
        } else if (found > 0 && ": ./-".indexOf(text.charAt(found - 1)) !== -1) {
            step += 4;
        }
        if (found === prev + 1) {
            run += 1;
            step += run * 2;
        } else {
            run = 0;
        }
        total += step;
        ti = found + 1;
        prev = found;
        first = false;
    }
    return total;
}

/* Catalog IDs are machine-scoped. Use names from the same machine as the
   pane, and retain the local list as a fallback for older proxy snapshots. */
function agentEntries(data) {
    var catalog = data.sessionCatalog || [];
    var groups = catalog.slice(0);
    var activeHost = data.activeHost || "";
    var activeFound = false;
    var rows = [];
    var i, j;
    for (i = 0; i < groups.length; i++) {
        if (groups[i].machine === activeHost && (groups[i].agent_panes || groups[i].unavailable)) { activeFound = true; }
    }
    if (!activeFound) {
        groups.push({machine:activeHost, agent_panes:data.agentPanes || [], sessions:[], tabs:[]});
    }
    for (i = 0; i < groups.length; i++) {
        var group = groups[i];
        if (group.unavailable) { continue; }
        var host = group.machine;
        var hostName = host;
        var hosts = data.hosts || [];
        for (j = 0; j < hosts.length; j++) {
            if (hosts[j].id === host) { hostName = hosts[j].name || host; }
        }
        var sessions = {};
        var tabs = {};
        for (j = 0; j < (group.sessions || []).length; j++) {
            sessions[group.sessions[j].id] = group.sessions[j].name;
        }
        for (j = 0; j < (group.tabs || []).length; j++) {
            tabs[group.tabs[j].id] = group.tabs[j].name;
        }
        var panes = group.agent_panes || [];
        for (j = 0; j < panes.length; j++) {
            var pane = panes[j];
            if (!pane.agent) { continue; }
            var sessionName = sessions[pane.session] ||
                (host === activeHost && (data.sessionNames || {})[pane.session]) || pane.session;
            var tabName = tabs[pane.tab] ||
                (host === activeHost && (data.tabNames || {})[pane.tab]) || pane.tab;
            var label = "Agent " + pane.agent.name + " — " + hostName + " / " + sessionName + " / " + tabName;
            rows.push({kind:"agent",host:host,sess:pane.session,idx:pane.tab,pane:pane.id,
                label:label,text:(label + " " + host + " " + pane.agent.state +
                    (pane.agent.state === "blocked" ? " needs input" : "")).toLowerCase(),
                agents:{state:pane.agent.state,count:1},
                active:host === activeHost && pane.id === currentPaneInfo(data).id});
        }
    }
    return rows;
}

var menuGroup = "sessions";
var menuQueries = {};
var menuGroups = ["hosts", "sessions", "panes", "agents"];

function renderMenuGroups(data) {
    for (var i = 0; i < menuGroups.length; i++) {
        var group = menuGroups[i];
        var button = byId("menu-group-" + group);
        if (button) {
            button.className = "menu-group" + (group === menuGroup ? " active" : "");
            button.setAttribute("aria-selected", group === menuGroup ? "true" : "false");
        }
    }
    var search = byId("f-menu-search");
    if (search && search.setAttribute) {
        search.setAttribute("placeholder", menuGroup === "panes" ?
            "Search panes across hosts..." : "Search " + menuGroup + "...");
    }
    var list = byId("menu-list");
    if (list && list.setAttribute) { list.setAttribute("aria-labelledby", "menu-group-" + menuGroup); }
}

function setMenuGroup(group) {
    if (menuGroups.indexOf(group) < 0 || group === menuGroup) { return; }
    var search = byId("f-menu-search");
    if (search) { menuQueries[menuGroup] = search.value; }
    menuGroup = group;
    if (search) { search.value = menuQueries[group] || ""; }
    lastMenuPick = 0;
    var list = byId("menu-list");
    if (list) { list.scrollTop = 0; }
    renderWindowMenu(lastData || {});
    setTimeout(function () {
        if (menuOpen && menuGroup === group && search) { search.focus(); }
    }, 0);
}

function wireMenuGroups() {
    var bar = byId("menu-groups");
    if (!bar) { return; }
    bar.addEventListener("mousedown", function (ev) {
        var node = ev.target || ev.srcElement;
        while (node && node !== bar) {
            var group = node.getAttribute && node.getAttribute("data-menu-group");
            if (group) {
                if (ev.preventDefault) { ev.preventDefault(); }
                if (ev.stopPropagation) { ev.stopPropagation(); }
                setMenuGroup(group);
                return;
            }
            node = node.parentNode;
        }
    });
}

function paneEntries(data) {
    var result = [];
    var panes = data.panes || [];
    var current = currentPaneInfo(data);
    var activeHost = data.activeHost || "";
    var hosts = data.hosts || [];
    var catalog = data.sessionCatalog || [];
    for (var i = 0; i < panes.length; i++) {
        var parts = panes[i].split(/\s+/);
        if (parts[0] !== "P" || parts.length < 6) { continue; }
        var label = hostDisplayName(data) + " / " +
            ((data.sessionNames || {})[parts[1]] || parts[1]) + " / " +
            ((data.tabNames || {})[parts[2]] || parts[2]) + " — " + parts[5];
        result.push({kind:"pane", host:data.activeHost || "", sess:parts[1], idx:parts[2], pane:parts[3],
            label:label, text:label.toLowerCase(),
            active:parts[1] === data.session && parts[3] === current.id});
    }
    for (i = 0; i < catalog.length; i++) {
        var group = catalog[i];
        if (!group.machine || group.machine === activeHost || group.unavailable) { continue; }
        var hostName = group.machine;
        for (var h = 0; h < hosts.length; h++) {
            if (hosts[h].id === group.machine) { hostName = hosts[h].name || group.machine; break; }
        }
        var sessions = {};
        var tabs = {};
        for (var s = 0; s < (group.sessions || []).length; s++) {
            sessions[group.sessions[s].id] = group.sessions[s].name;
        }
        for (var t = 0; t < (group.tabs || []).length; t++) {
            tabs[group.tabs[t].id] = group.tabs[t].name;
        }
        var remotePanes = group.panes || [];
        for (var p = 0; p < remotePanes.length; p++) {
            var pane = remotePanes[p];
            var sessionName = sessions[pane.session] || pane.session;
            var tabName = tabs[pane.tab] || pane.tab;
            var remoteLabel = hostName + " / " + sessionName + " / " + tabName +
                " — " + (pane.command || "");
            result.push({kind:"pane",host:group.machine,sess:pane.session,idx:pane.tab,pane:pane.id,
                label:remoteLabel,text:(remoteLabel + " " + group.machine).toLowerCase(),active:false});
        }
    }
    return result;
}

function renderWindowMenu(data) {
    var list = byId("menu-list");
    if (!list) { return; }
    var search = byId("f-menu-search");
    var raw = (search.value || "").replace(/^\s+|\s+$/g, "");
    // Retain the old search shortcut, but make it a real group selection.
    if (/^@agents(?:\s|$)/i.test(raw)) {
        menuGroup = "agents";
        raw = raw.replace(/^@agents\s*/i, "");
        search.value = raw;
    }
    renderMenuGroups(data);
    var q = raw.toLowerCase();
    var hosts = data.hosts || [];
    var catalog = data.sessionCatalog || [];
    var activeHost = data.activeHost || "";
    var items = [];
    var sessionNames = {};
    var i, j;
    var groups = {};
    for (i = 0; i < catalog.length; i++) { groups[catalog[i].machine] = catalog[i]; }
    if (menuGroup === "hosts") {
        for (i = 0; i < hosts.length; i++) {
            var host = hosts[i];
            var state = groups[host.id];
            var hostLabel = host.name || host.id;
            if (state && state.unavailable) { hostLabel += " — Unavailable"; }
            items.push({kind:"host",host:host.id,label:hostLabel,
                text:(host.id + " " + hostLabel).toLowerCase(),active:host.id === activeHost});
        }
    } else if (menuGroup === "sessions") {
        for (i = 0; i < catalog.length; i++) {
            var group = catalog[i];
            var machineName = group.machine;
            for (j = 0; j < hosts.length; j++) {
                if (hosts[j].id === group.machine) { machineName = hosts[j].name || group.machine; }
            }
            var entries = group.unavailable ? [] : (group.sessions || []);
            for (j = 0; j < entries.length; j++) {
                var entry = entries[j];
                var label = machineName + " / " + (entry.name || entry.id);
                if (group.machine === activeHost) { sessionNames[entry.name || entry.id] = true; }
                items.push({kind:"session",host:group.machine,sess:entry.id,label:label,
                    text:(group.machine + " " + label).toLowerCase(),agents:entry.agents,
                    active:group.machine === activeHost && entry.id === data.session});
            }
        }
        // Older catalog snapshots may omit the active session names.
        var sessions = data.sessions || [];
        for (i = 0; i < sessions.length; i++) {
            var parts = sessions[i].split(/\s+/);
            if (parts[0] === "S") { sessionNames[(data.sessionNames || {})[parts[1]] || parts[1]] = true; }
        }
    } else if (menuGroup === "panes") {
        items = paneEntries(data);
    } else {
        items = agentEntries(data);
    }
    var shown = [];
    for (i = 0; i < items.length; i++) {
        items[i].order = i;
        var score = q ? fuzzyScore(q, items[i].text) : 1;
        if (score) { shown.push({score:score,item:items[i]}); }
    }
    if (q) {
        shown.sort(function (a, b) { return b.score - a.score || a.item.order - b.item.order; });
    }
    menuRows = [];
    var html = "";
    for (i = 0; i < shown.length; i++) {
        var it = shown[i].item;
        menuRows.push(it);
        var kind = it.kind === "session" ? "host-session" : it.kind;
        var id = it.kind === "host" ? it.host : (it.kind === "session" ? it.sess : it.pane);
        html += '<div class="win-menu-item' + (it.active ? " active" : "") +
            '" data-kind="' + kind + '" data-host="' + esc(it.host || activeHost) +
            '" data-session="' + esc(it.sess || "") + '" data-tab="' + esc(it.idx || "") +
            '" data-id="' + esc(id) + '">' +
            '<span class="win-ico">' + (it.active ? ICON_ACTIVE : "\u00A0") + "</span>" +
            '<span class="win-ico">' + menuKindIcon(it.kind) + "</span>" +
            esc(it.label) + agentBadge(it.agents) + "</div>";
    }
    if (menuGroup === "sessions" && raw && validSessionName(raw) && !sessionNames[raw]) {
        menuRows.push({create:true,name:raw});
        html += '<div class="win-menu-item create" data-kind="create-session" data-id="' + esc(raw) +
            '"><span class="win-ico">' + ICON_SESSION + "</span>Create session " + esc(raw) + "</div>";
    }
    if (!menuRows.length && !html) {
        if (q) { html = '<div class="empty">No matches</div>'; }
        else if (menuGroup === "hosts" && !hosts.length) {
            html = '<div class="empty">No hosts configured</div>';
        }
    }
    list.innerHTML = html;
}

function hostDisplayName(data) {
    var hosts = (data && data.hosts) || [];
    var id = (data && data.activeHost) || "";
    var i;
    for (i = 0; i < hosts.length; i++) {
        if (hosts[i].id === id) {
            return hosts[i].name || hosts[i].id;
        }
    }
    if (hosts.length === 1) {
        return hosts[0].name || hosts[0].id;
    }
    return id;
}

function jumpToHost(id) {
    if (!id) {
        return;
    }
    beginPaneSwitch();
    awaitHost = id;
    sendCmd({ op: "host", id: id });
    closeWinMenu();
    showView("view");
}

function jumpToHostSession(host, session) {
    if (!host || !session) { return; }
    beginPaneSwitch();
    awaitHost = host;
    // One daemon command selects the exact session; never race a host switch
    // against a second queued selection with machine-local IDs.
    sendCmd({ op: "host_session", id: host, session: session });
    closeWinMenu();
    showView("view");
}

/* Scrollback paging is owned by the WAF over the captured history,
   except in tmux copy mode: then page keys go to the pane (send-keys -X)
   and the daemon shows capture-pane -M. Live is scrollBottom (or paging
   down to offset 0); in copy mode that cancels the mode. */
function scrollPage() {
    return (lastData && lastData.rows) ? lastData.rows : 24;
}

function scrollMax() {
    return (lastData && lastData.scrollback) ? lastData.scrollback.length : 0;
}

function scrollUp() {
    if (lastData && lastData.copyMode) {
        sendCmd({ op: "key", key: "ScrollUp" });
        return;
    }
    scrollToOffset(Math.min(scrollMax(), viewOffset + scrollPage()));
}

function scrollDown() {
    if (lastData && lastData.copyMode) {
        sendCmd({ op: "key", key: "ScrollDown" });
        return;
    }
    scrollToOffset(Math.max(0, viewOffset - scrollPage()));
}

function scrollBottom() {
    cmdHistIdx = -1;
    if (lastData && lastData.copyMode) {
        sendCmd({ op: "key", key: "ScrollBottom" });
        return;
    }
    scrollToOffset(0);
}

function resetToLive() {
    cmdHistIdx = -1;
    viewOffset = 0;
    lastDaemonOffset = 0;
    scrollAckUntil = 0;
    pendingRestore = null;
    switchPending = false;
    awaitHost = "";
    if (lastPaneKey) {
        paneOffsets[lastPaneKey] = { off: 0, sb: lastSbLen };
    }
}

/* Save this pane's scrollback, then show Live immediately so the 80x24
   frame never collapses while switch/create/close recaptures. Destination
   offset is restored when its pane id arrives. */
function beginPaneSwitch() {
    if (lastPaneKey && lastSbLen > 0) {
        paneOffsets[lastPaneKey] = { off: viewOffset, sb: lastSbLen };
    }
    viewOffset = 0;
    lastDaemonOffset = 0;
    cmdHistIdx = -1;
    pendingRestore = { off: 0, sb: -1 };
    switchPending = true;
}

/* Keyboard chrome and the Kindle VKB stay up together under the
   terminal. kbWanted is true while #f-text (or another input) holds
   the VKB. */
var kbPanel = "scroll";
var kbWanted = true;
var modCtrl = false;
var modAlt = false;
var promptRulesFile = null;
var quickSnippets = [];
var lastPromptSig = "";
/* The VKB delivers special keys either as real key events or — on this
   old WebKit — as the key's label text committed into the capture field
   (e.g. the value becomes "Backspace"). Route both to the daemon's key op. */
var CAPTURE_KEYS = {
    "Backspace": "Backspace", "Delete": "Delete",
    "Enter": "Enter", "Tab": "Tab", "Escape": "Escape", "Esc": "Escape",
    "Up": "Up", "Down": "Down", "Left": "Left", "Right": "Right",
    "Home": "Home", "End": "End", "PageUp": "PageUp", "PageDown": "PageDown"
};

function kbChromeEl() {
    return byId("kb-chrome");
}

function focusIsInKbChrome() {
    var a = document.activeElement;
    var chrome = kbChromeEl();
    if (!a || !chrome) {
        return false;
    }
    if (a.id === "f-text") {
        return true;
    }
    while (a && a !== document) {
        if (a === chrome) {
            return true;
        }
        a = a.parentNode;
    }
    return false;
}

function otherInputFocused() {
    var a = document.activeElement;
    var tag = a && a.tagName ? a.tagName.toLowerCase() : "";
    return !!(a && (tag === "input" || tag === "textarea") && a.id !== "f-text");
}

/* Keep the Kindle keyboard up by holding focus on #f-text, unless the
   user is typing in settings / window search / chrome search. */
function holdVkb() {
    kbWanted = true;
    if (composeVisible() || otherInputFocused()) {
        return;
    }
    var f = byId("f-text");
    if (!f) {
        return;
    }
    setTimeout(function () {
        if (composeVisible() || otherInputFocused()) {
            return;
        }
        if (f && document.activeElement !== f) {
            f.focus();
        }
    }, 0);
}

function showKbChrome() {
    holdVkb();
}

function hideKbChrome() {
    clearMods();
    holdVkb();
}

function showPanel(name) {
    if (name === "ctrl" || name === "ai" || name === "custom") {
        name = "prompt";
    }
    if (name === "tmux") {
        name = "scroll";
    }
    kbPanel = name || "scroll";
    var ids = ["scroll", "prompt", "tools"];
    var i;
    for (i = 0; i < ids.length; i++) {
        var pan = byId("panel-" + ids[i]);
        if (pan) {
            pan.style.display = ids[i] === kbPanel ? "" : "none";
        }
    }
    var tabs = byId("kb-stabs");
    if (tabs) {
        var btns = tabs.getElementsByTagName("button");
        for (i = 0; i < btns.length; i++) {
            var on = btns[i].getAttribute("data-panel") === kbPanel;
            btns[i].className = on ? "active" : "";
        }
    }
    if (kbPanel === "prompt") {
        renderPromptPanel();
    }
}

function clearMods() {
    modCtrl = false;
    modAlt = false;
    paintMods();
}

function toggleMod(name) {
    if (name === "ctrl") {
        modCtrl = !modCtrl;
    } else if (name === "alt") {
        modAlt = !modAlt;
    }
    paintMods();
}

function paintMods() {
    var bar = byId("kb-toolbar");
    if (!bar) {
        return;
    }
    var btns = bar.getElementsByTagName("button");
    var i;
    for (i = 0; i < btns.length; i++) {
        var m = btns[i].getAttribute("data-mod");
        if (!m) {
            continue;
        }
        var on = (m === "ctrl" && modCtrl) || (m === "alt" && modAlt);
        btns[i].className = on ? "active" : "";
    }
}

/* Named keys stay tmux names (Up, Escape, PageUp). Letters lowercased. */
function tmuxToken(key) {
    if (!key) {
        return "";
    }
    if (key === "Esc") {
        return "Escape";
    }
    if (CAPTURE_KEYS[key]) {
        return CAPTURE_KEYS[key];
    }
    if (key.length === 1) {
        return key.toLowerCase();
    }
    return key;
}

function hasKeyPrefix(key) {
    return key.indexOf("C-") === 0 || key.indexOf("M-") === 0 ||
        key.indexOf("S-") === 0 || key.indexOf("K:") === 0;
}

function selectionTextFromScreen(data) {
    if (!data || !data.screen) {
        return "";
    }
    var rows = data.screen;
    var attrs = data.attrs || [];
    var lines = [];
    var y;
    for (y = 0; y < rows.length; y++) {
        var t = rows[y] || "";
        var a = attrs[y] || "";
        var buf = "";
        var x;
        for (x = 0; x < t.length; x++) {
            var ch = a.charAt(x);
            if (ch === "i" || ch === "B") {
                buf += t.charAt(x);
            }
        }
        if (buf) {
            lines.push(buf.replace(/\s+$/, ""));
        }
    }
    return lines.join("\n");
}

function copyToKindleClipboard(text) {
    if (!text) {
        return;
    }
    var ta = byId("clip-ta");
    if (!ta) {
        return;
    }
    var f = byId("f-text");
    var restore = f && document.activeElement === f;
    ta.value = text;
    try {
        ta.focus();
        if (ta.select) {
            ta.select();
        }
        document.execCommand("copy");
    } catch (err) {}
    if (restore) {
        keepCaptureFocus();
    }
}

function maybeYankClipboard(kind, value) {
    if (!lastData || !lastData.copyMode) {
        return;
    }
    var yank = false;
    if (kind === "key" && (value === "Enter" || value === "y" || value === "C-j")) {
        yank = true;
    }
    if (kind === "text" && (value === "y" || value === "\n" || value === "\r")) {
        yank = true;
    }
    if (!yank) {
        return;
    }
    var t = selectionTextFromScreen(lastData);
    if (t) {
        copyToKindleClipboard(t);
    }
}

/* Apply sticky Ctrl/Alt to a tmux key name ("c", "Up", "C-c"). */
function sendWithMods(kind, value) {
    if (kind === "key" && (value === "Escape" || value === "Esc") && dismissVisibleOverlay()) {
        return;
    }
    maybeYankClipboard(kind, value);
    var key = value;
    if (kind === "text" && value && value.length === 1) {
        if (modCtrl || modAlt) {
            kind = "key";
            key = tmuxToken(value);
        }
    }
    if (kind === "key") {
        if (!hasKeyPrefix(key)) {
            if (modCtrl && modAlt) {
                key = "C-M-" + tmuxToken(key);
            } else if (modCtrl) {
                key = "C-" + tmuxToken(key);
            } else if (modAlt) {
                key = "M-" + tmuxToken(key);
            }
        }
        sendCmd({ op: "key", key: key });
        if (modCtrl || modAlt) {
            clearMods();
        }
        keepCaptureFocus();
        return;
    }
    sendCmd({ op: "text", text: value });
    keepCaptureFocus();
}

function keepCaptureFocus() {
    holdVkb();
}

function sendSlash(cmd) {
    sendCmd({ op: "text", text: cmd });
    sendCmd({ op: "key", key: "Enter" });
    keepCaptureFocus();
}

/* Custom shortcut sequence: space-separated tokens. "C-c" is a key,
   "/foo" is text+Enter, anything else is literal text. */
function sendSequence(seq) {
    var s = (seq || "").replace(/^\s+|\s+$/g, "");
    if (!s) {
        return;
    }
    if (s.charAt(0) === "/" && s.indexOf(" ") < 0) {
        sendSlash(s);
        return;
    }
    var parts = s.split(/\s+/);
    var i;
    for (i = 0; i < parts.length; i++) {
        var t = parts[i];
        if (CAPTURE_KEYS[t] || /^(C|M|S)-/.test(t) || t.indexOf("K:") === 0) {
            sendCmd({ op: "key", key: CAPTURE_KEYS[t] || t });
        } else {
            sendCmd({ op: "text", text: t });
        }
    }
    keepCaptureFocus();
}

/* Cell indexes in `text` that belong to a case-insensitive match. */
function hitCells(text, needle) {
    var out = {};
    if (!needle || !text) {
        return out;
    }
    var cells = scalars(text);
    var lows = [];
    var i;
    for (i = 0; i < cells.length; i++) {
        lows.push(cells[i].toLowerCase());
    }
    var joined = lows.join("");
    var from = 0;
    var p;
    while ((p = joined.indexOf(needle, from)) !== -1) {
        var cell = 0;
        var pos = 0;
        while (cell < lows.length && pos + lows[cell].length <= p) {
            pos += lows[cell].length;
            cell += 1;
        }
        var endPos = p + needle.length;
        var j = cell;
        var pos2 = pos;
        while (j < lows.length && pos2 < endPos) {
            out[j] = true;
            pos2 += lows[j].length;
            j += 1;
        }
        from = p + 1;
        if (!needle.length) {
            break;
        }
    }
    return out;
}

function searchScrollbackBack() {
    searchScrollback("back");
}

function searchScrollbackFwd() {
    searchScrollback("fwd");
}

function pageHasMatch(hist, start, n, needle) {
    var i;
    var end = start + n;
    if (start < 0) {
        start = 0;
    }
    if (end > hist.length) {
        end = hist.length;
    }
    for (i = start; i < end; i++) {
        if (String(hist[i] || "").toLowerCase().indexOf(needle) !== -1) {
            return true;
        }
    }
    return false;
}

/* dir "back" = older history; "fwd" = toward live. Move a full screen
   (24 lines) at a time until a page contains a match, then highlight. */
function searchScrollback(dir) {
    var box = byId("f-scroll-search");
    var q = box ? (box.value || "").replace(/^\s+|\s+$/g, "") : "";
    if (q && lastData && lastData.copyMode) {
        sendCmd({ op: "copy_search", query: q, backwards: dir === "back" });
        return;
    }
    if (!q) {
        searchNeedle = "";
        if (lastData) {
            safeRender(lastData);
        }
        return;
    }
    searchNeedle = q.toLowerCase();
    if (!lastData) {
        return;
    }
    var rows = lastData.screen || [];
    var sb = lastData.scrollback || [];
    var hist = sb.slice().reverse().concat(rows);
    var total = hist.length;
    var rowN = lastData.rows || rows.length || 24;
    var maxOff = sb.length;
    var off = viewOffset;
    var next;
    if (dir === "fwd") {
        while (off > 0) {
            next = off - rowN;
            if (next < 0) {
                next = 0;
            }
            if (pageHasMatch(hist, total - next - rowN, rowN, searchNeedle)) {
                scrollToOffset(next);
                return;
            }
            if (next === off) {
                break;
            }
            off = next;
        }
    } else {
        while (off < maxOff) {
            next = off + rowN;
            if (next > maxOff) {
                next = maxOff;
            }
            if (pageHasMatch(hist, total - next - rowN, rowN, searchNeedle)) {
                scrollToOffset(next);
                return;
            }
            if (next === off) {
                break;
            }
            off = next;
        }
    }
    safeRender(lastData);
}

function clearScrollSearch() {
    var box = byId("f-scroll-search");
    if (box) {
        box.value = "";
    }
    searchNeedle = "";
    if (lastData) {
        safeRender(lastData);
    }
    keepCaptureFocus();
}

/* Fish prompt as captured by kmux: `❯` (U+276F) + space, then the
   command. Desktop tmux copy-mode matches Nerd Font `` + nbsp; accept
   both, plus a couple of common prompt arrows. */
function isPromptGlyph(ch) {
    return ch === "\u276F" || ch === "\uF054" || ch === "\u276D" ||
        ch === "\u25B6";
}

function promptCmdHits(text) {
    var hits = {};
    var cells = scalars(text || "");
    var i;
    var start = -1;
    for (i = 0; i < cells.length; i++) {
        if (isPromptGlyph(cells[i])) {
            start = i + 1;
            if (start < cells.length &&
                    (cells[start] === "\u00A0" || cells[start] === " ")) {
                start += 1;
            }
            break;
        }
    }
    if (start < 0) {
        return null;
    }
    var end = cells.length;
    while (end > start && (cells[end - 1] === " " || cells[end - 1] === "\u00A0")) {
        end -= 1;
    }
    if (end <= start) {
        return null;
    }
    for (i = start; i < end; i++) {
        hits[i] = true;
    }
    return hits;
}

function jumpToCmd(histIdx, total, rowN) {
    cmdHistIdx = histIdx;
    var off = total - (histIdx + rowN);
    if (off < 0) {
        off = 0;
    }
    var max = (lastData && lastData.scrollback) ? lastData.scrollback.length : 0;
    if (off > max) {
        off = max;
    }
    scrollToOffset(off);
}

/* Same motion as tmux M-Up / M-Down: previous/next command after a
   shell prompt, then highlight that command. */
function selectCmd(dir) {
    if (!lastData) {
        return;
    }
    var rows = lastData.screen || [];
    var sb = lastData.scrollback || [];
    var hist = sb.slice().reverse().concat(rows);
    var total = hist.length;
    var rowN = lastData.rows || rows.length || 24;
    var i;
    var start;
    if (dir === "prev") {
        start = cmdHistIdx >= 0 ? cmdHistIdx - 1 : (total - viewOffset - 1);
        for (i = start; i >= 0; i--) {
            if (promptCmdHits(hist[i])) {
                jumpToCmd(i, total, rowN);
                keepCaptureFocus();
                return;
            }
        }
    } else {
        start = cmdHistIdx >= 0 ? cmdHistIdx + 1 : (total - viewOffset - rowN);
        for (i = start; i < total; i++) {
            if (promptCmdHits(hist[i])) {
                jumpToCmd(i, total, rowN);
                keepCaptureFocus();
                return;
            }
        }
        cmdHistIdx = -1;
        scrollBottom();
    }
    keepCaptureFocus();
}

/* `P sess win paneId paneIdx command [WxH] active` */
function currentPaneInfo(data) {
    var info = { cmd: "", win: "", id: "", winIdx: "" };
    if (!data) {
        return info;
    }
    var sess = data.session || "";
    var wins = data.windows || [];
    var panes = data.panes || [];
    var winIdx = "";
    var i;
    for (i = 0; i < wins.length; i++) {
        var w = wins[i].split(/\s+/);
        if (w.length >= 4 && w[0] === "W" && w[1] === sess &&
                wins[i].indexOf("*") !== -1) {
            winIdx = w[2];
            info.winIdx = winIdx;
            info.win = w[3] || "";
            break;
        }
    }
    for (i = 0; i < panes.length; i++) {
        var p = panes[i].split(/\s+/);
        if (p.length < 8 || p[0] !== "P" || p[1] !== sess) {
            continue;
        }
        if (winIdx && p[2] !== winIdx) {
            continue;
        }
        if (p[p.length - 1] !== "1") {
            continue;
        }
        info.id = p[3] || "";
        info.cmd = p[5] || "";
        break;
    }
    return info;
}

function paneKey(data) {
    var host = (data && data.activeHost) || "";
    var sess = (data && data.session) || "";
    var info = currentPaneInfo(data);
    var prefix = host ? (host + "/") : "";
    if (info.id) {
        return prefix + sess + ":" + info.id;
    }
    if (info.winIdx) {
        return prefix + sess + ":w:" + info.winIdx;
    }
    return prefix;
}

function screenHasContent(rows, sb) {
    if (sb && sb.length) {
        return true;
    }
    var i;
    for (i = 0; i < (rows || []).length; i++) {
        if (String(rows[i] || "").replace(/[\s\u00a0]/g, "") !== "") {
            return true;
        }
    }
    return false;
}

function ruleMatches(rule, info) {
    var m = (rule && rule.match) ? rule.match : [];
    var cmd = ((info && info.cmd) || "").toLowerCase();
    var win = ((info && info.win) || "").toLowerCase();
    var i;
    for (i = 0; i < m.length; i++) {
        var t = String(m[i] || "").toLowerCase();
        if (!t) {
            continue;
        }
        if (t === "*") {
            return true;
        }
        if (cmd === t || (cmd && cmd.indexOf(t) === 0)) {
            return true;
        }
        if (win === t || (win && win.indexOf(t) === 0)) {
            return true;
        }
    }
    return false;
}

function pickPromptRule(info) {
    var file = promptRulesFile || {};
    var rules = file.rules || [];
    var i;
    for (i = 0; i < rules.length; i++) {
        if (ruleMatches(rules[i], info)) {
            return rules[i];
        }
    }
    var fb = file.fallback || "shell";
    for (i = 0; i < rules.length; i++) {
        if (rules[i].id === fb) {
            return rules[i];
        }
    }
    return rules.length ? rules[0] : { keys: [] };
}

function promptButtonHtml(k) {
    if (!k || !k.label) {
        return "";
    }
    var label = k.label;
    /* Ctrl chords show the control-key mark (caret) before the letter. */
    if (k.key && k.key.indexOf("C-") === 0 &&
            label.charAt(0) !== "^" && label.charAt(0) !== "\u2303") {
        label = "^" + label;
    }
    var html = '<button type="button"';
    var title = k.title || k.key || k.slash || k.seq || k.text || k.prompt || "";
    if (title) {
        html += ' title="' + esc(title) + '"';
    }
    if (k.key) {
        html += ' data-key="' + esc(k.key) + '"';
    } else if (k.slash) {
        html += ' data-slash="' + esc(k.slash) + '"';
    } else if (k.prompt) {
        html += ' data-prompt="' + esc(k.prompt) + '"';
    } else if (k.text) {
        html += ' data-text="' + esc(k.text) + '"';
    } else if (k.seq) {
        html += ' data-seq="' + esc(k.seq) + '"';
    }
    html += ">" + esc(label) + "</button>";
    return html;
}

function renderPromptPanel() {
    var host = byId("prompt-list");
    if (!host) {
        return;
    }
    var info = currentPaneInfo(lastData);
    var rule = pickPromptRule(info);
    var keys = (rule && rule.keys) ? rule.keys : [];
    var html = "";
    var i;
    for (i = 0; i < keys.length; i++) {
        html += promptButtonHtml(keys[i]);
    }
    host.innerHTML = html;
}

function fetchPromptRules() {
    var xhr = new XMLHttpRequest();
    try {
        xhr.open("GET", "prompt-rules.json?t=" + new Date().getTime(), true);
    } catch (err) {
        renderPromptPanel();
        return;
    }
    xhr.onreadystatechange = function () {
        if (xhr.readyState !== 4) {
            return;
        }
        if (xhr.status === 200 || xhr.status === 0) {
            try {
                var obj = JSON.parse(xhr.responseText);
                if (obj && obj.rules) {
                    promptRulesFile = obj;
                }
            } catch (e) {}
        }
        renderPromptPanel();
    };
    xhr.send(null);
}

function preventChromeBlur(ev) {
    if (ev.preventDefault) {
        ev.preventDefault();
    }
}

function closestButton(node) {
    while (node && node !== document) {
        var tag = node.tagName ? node.tagName.toLowerCase() : "";
        if (tag === "button") {
            return node;
        }
        /* Window tabs and the session chip are spans, not <button>. */
        var cls = " " + (node.className || "") + " ";
        if (cls.indexOf(" wintab ") !== -1) {
            return node;
        }
        node = node.parentNode;
    }
    return null;
}

/* Invert for a beat so a tap is visible on e-ink. Inline styles so a
   later className rewrite (Ctrl latch, tab active) does not cancel it.
   Timeout instead of mouseup: this WebKit often never sends mouseup.
   The session chip's .active is fill-inverted too; treat it like a tab. */
function flashPressed(el) {
    if (!el) {
        return;
    }
    var cls = " " + (el.className || "") + " ";
    var on = cls.indexOf(" active ") >= 0;
    el.style.backgroundColor = on ? "#fff" : "#000";
    el.style.color = on ? "#000" : "#fff";
    setTimeout(function () {
        el.style.backgroundColor = "";
        el.style.color = "";
    }, PRESS_FLASH_MS);
}

/* Run after the invert has been on screen (Settings/Exit/session chip
   change class or leave the app in the same tap). */
function afterPressFlash(fn) {
    setTimeout(fn, PRESS_FLASH_MS);
}

function wireKbChrome() {
    var chrome = kbChromeEl();
    if (!chrome) {
        return;
    }
    chrome.addEventListener("mousedown", function (ev) {
        var node = ev.target || ev.srcElement;
        var tname = node && node.tagName ? node.tagName.toLowerCase() : "";
        if (tname !== "input") {
            preventChromeBlur(ev);
        }
        while (node && node !== chrome) {
            var panel = node.getAttribute && node.getAttribute("data-panel");
            var op = node.getAttribute && node.getAttribute("data-op");
            var key = node.getAttribute && node.getAttribute("data-key");
            var text = node.getAttribute && node.getAttribute("data-text");
            var slash = node.getAttribute && node.getAttribute("data-slash");
            var seq = node.getAttribute && node.getAttribute("data-seq");
            var mod = node.getAttribute && node.getAttribute("data-mod");
            var scroll = node.getAttribute && node.getAttribute("data-scroll");
            var act = node.getAttribute && node.getAttribute("data-act");
            var prompt = node.getAttribute && node.getAttribute("data-prompt");
            if (panel) {
                showPanel(panel);
                keepCaptureFocus();
                return;
            }
            if (mod) {
                toggleMod(mod);
                keepCaptureFocus();
                return;
            }
            if (op) {
                runWindowOp(op);
                keepCaptureFocus();
                return;
            }
            if (scroll) {
                if (scroll === "up") {
                    scrollUp();
                } else if (scroll === "down") {
                    scrollDown();
                } else if (scroll === "bottom") {
                    scrollBottom();
                }
                keepCaptureFocus();
                return;
            }
            if (act === "search-back") {
                searchScrollback("back");
                return;
            }
            if (act === "search-fwd") {
                searchScrollback("fwd");
                return;
            }
            if (act === "search-clear") {
                clearScrollSearch();
                return;
            }
            if (act === "files") {
                toggleFilePicker();
                return;
            }
            if (act === "paste") {
                afterPressFlash(showPastePop);
                keepCaptureFocus();
                return;
            }
            if (act === "snippets") {
                afterPressFlash(showSnippetPop);
                keepCaptureFocus();
                return;
            }
            if (act === "cmd-prev") {
                selectCmd("prev");
                return;
            }
            if (act === "cmd-next") {
                selectCmd("next");
                return;
            }
            if (key) {
                sendWithMods("key", key);
                return;
            }
            if (text) {
                sendWithMods("text", text);
                return;
            }
            if (slash) {
                sendSlash(slash);
                return;
            }
            if (prompt) {
                sendCmd({ op: "text", text: prompt });
                sendCmd({ op: "key", key: "Enter" });
                keepCaptureFocus();
                return;
            }
            if (seq) {
                sendSequence(seq);
                return;
            }
            node = node.parentNode;
        }
    });
    var sbox = byId("f-scroll-search");
    if (sbox) {
        sbox.addEventListener("keydown", function (ev) {
            var k = ev.keyCode || ev.which;
            if (k === 13) {
                searchScrollbackBack();
                if (ev.preventDefault) {
                    ev.preventDefault();
                }
                return false;
            }
            return true;
        });
        sbox.addEventListener("focus", showKbChrome);
    }
    showPanel(kbPanel);
}

/* True only on the plain "Live" state: connected, live screen, no alt
   screen. The terminal keyboard is only usable in this state, so it is
   never raised otherwise. */
function isLiveNow() {
    var d = lastData;
    return !!(d && d.connected && !d.altScreen && viewOffset === 0);
}

var currentView = "view";
var menuOpen = false;
var actMenuOpen = false;
var filePickerOpen = false;
/* Pane cwd when the picker opened; insert paths are relative to this. */
var pickerRoot = "";
var lastPickerCwd = "";
var pickerAwaitingRoot = false;
var fileRows = [];
/* Rows of the last dropdown render (filtered + sorted, create row last):
   what the search box's Enter key picks from. */
var menuRows = [];
var lastMenuPick = 0;

/* Switch the daemon to (session, window) and return to the terminal.
   Used by the window dropdown rows. */
function jumpToWindow(session, index) {
    beginPaneSwitch();
    sendCmd({ op: "window", index: index, session: session });
    showView("view");
}

/* Enter in the dropdown search: prefer the active (currently displayed)
   window when it is among the filtered results; otherwise take the first
   result. */
function pickMenuResult() {
    var now = new Date().getTime();
    if (now - lastMenuPick < 500) {
        return; /* keydown + keyup both report the same Enter */
    }
    lastMenuPick = now;
    var best = null;
    var i;
    for (i = 0; i < menuRows.length; i++) {
        if (menuRows[i].active) {
            best = menuRows[i];
            break;
        }
    }
    if (!best && menuRows.length) {
        best = menuRows[0];
    }
    if (best) {
        if (best.create) {
            createSessionByName(best.name);
        } else if (best.kind === "host") {
            jumpToHost(best.host);
        } else if (best.kind === "session") {
            jumpToHostSession(best.host, best.sess);
        } else if (best.kind === "agent") {
            jumpToAgent(best.host, best.sess, best.idx, best.pane);
        } else if (best.kind === "pane") {
            jumpToPane(best.host, best.sess, best.idx, best.pane);
        } else {
            jumpToWindow(best.sess, best.idx);
        }
    }
}

/* Create a tmux session and show it (the dropdown search offers this
   when the typed name matches no existing session). */
function createSessionByName(name) {
    if (!validSessionName(name)) {
        return;
    }
    beginPaneSwitch();
    sendCmd({ op: "new_session", name: name });
    closeWinMenu();
}

/* Window dropdown: open renders the list from the last status and pulls a
   fresh one (the 2.5s poll keeps it live while open); close restores the
   chip. The dropdown overlays the terminal, so the view never changes. */
function jumpToPane(host, session, tab, pane) {
    if (!lastData || !host || !session || !tab || !pane) { return; }
    var entries = paneEntries(lastData);
    for (var i = 0; i < entries.length; i++) {
        if (entries[i].host === host && entries[i].sess === session &&
                entries[i].idx === tab && entries[i].pane === pane) {
            beginPaneSwitch();
            closeWinMenu();
            showView("view");
            if (host === lastData.activeHost) {
                sendCmd({op:"pane",session:session,win:tab,id:pane,expected_machine:host});
            } else {
                awaitHost = host;
                sendCmd({op:"host_pane",id:host,session:session,tab:tab,pane:pane});
            }
            return;
        }
    }
}

function jumpToAgent(host, session, tab, pane) {
    if (!lastData || !host || !session || !tab || !pane) { return; }
    var entries = agentEntries(lastData);
    var found = false;
    for (var i = 0; i < entries.length; i++) {
        if (entries[i].host === host && entries[i].sess === session && entries[i].idx === tab && entries[i].pane === pane) {
            found = true; break;
        }
    }
    if (!found) { return; } // Closed/offline/stale targets must not redirect input.
    beginPaneSwitch();
    closeWinMenu();
    showView("view");
    if (host === lastData.activeHost) {
        sendCmd({ op: "pane", session: session, win: tab, id: pane, expected_machine: host });
    } else {
        awaitHost = host;
        sendCmd({ op: "host_pane", id: host, session: session, tab: tab, pane: pane });
    }
}

function openWinMenu() {
    prepareOverlay("win-menu");
    var title = byId("menu-title");
    if (title) { title.innerHTML = "Switch to"; }
    menuGroup = "sessions";
    menuQueries = {};
    var menu = byId("win-menu");
    if (!menu) {
        return;
    }
    var f = byId("f-text");
    if (f && document.activeElement === f) {
        f.blur(); /* hand focus off the terminal capture field */
    }
    clearMods();
    /* A stale search keyword must not survive into the next dropdown. */
    var search = byId("f-menu-search");
    if (search) {
        search.value = "";
    }
    menuOpen = true;
    var chip = byId("session-chip");
    if (chip) {
        chip.className = "wintab wintab-session active";
    }
    renderWindowMenu(lastData || {});
    menu.style.display = "";
    sendCmd({ op: "list" });
    pokeScreen();
    /* Focus the search box (deferred: a focus right after a tap is
       swallowed by this WebKit) so the keyboard is ready to type. */
    setTimeout(function () {
        var fs = byId("f-menu-search");
        if (menuOpen && fs && document.activeElement !== fs) {
            fs.focus();
        }
    }, 0);
}

function closeWinMenu() {
    menuOpen = false;
    var menu = byId("win-menu");
    if (menu) {
        menu.style.display = "none";
    }
    var chip = byId("session-chip");
    if (chip) {
        chip.className = "wintab wintab-session";
    }
    var search = byId("f-menu-search");
    if (search && document.activeElement === search) {
        search.blur();
    }
    holdVkb();
}

function toggleWinMenu() {
    if (menuOpen) {
        closeWinMenu();
    } else {
        openWinMenu();
    }
}

function openActMenu() {
    prepareOverlay("act-menu");
    var menu = byId("act-menu");
    if (!menu) {
        return;
    }
    var f = byId("f-text");
    if (f && document.activeElement === f) {
        f.blur();
    }
    clearMods();
    actMenuOpen = true;
    var btn = byId("act-more");
    if (btn) {
        btn.className = "wintab wintab-new active";
    }
    menu.style.display = "";
}

function closeActMenu() {
    actMenuOpen = false;
    var menu = byId("act-menu");
    if (menu) {
        menu.style.display = "none";
    }
    var btn = byId("act-more");
    if (btn) {
        btn.className = "wintab wintab-new";
    }
}

function toggleActMenu() {
    if (actMenuOpen) {
        closeActMenu();
    } else {
        openActMenu();
    }
}

function runWindowOp(op) {
    if (!op) {
        return;
    }
    closeActMenu();
    closeWinMenu();
    if (op !== "detach" && op !== "copy_mode") {
        beginPaneSwitch();
    }
    sendCmd({ op: op });
    if (op === "detach") {
        resetToLive();
    }
    showView("view");
}

function isSafeShellWord(s) {
    return s && /^[A-Za-z0-9._/+\-]+$/.test(s);
}

function shellQuote(s) {
    s = String(s || "");
    if (isSafeShellWord(s)) {
        return s;
    }
    return "'" + s.replace(/'/g, "'\\''") + "'";
}

function joinPath(dir, name) {
    if (name === "..") {
        if (!dir || dir === "/") {
            return "/";
        }
        var i = dir.lastIndexOf("/");
        if (i <= 0) {
            return "/";
        }
        return dir.substring(0, i);
    }
    if (!dir || dir === "/") {
        return "/" + name;
    }
    return dir + "/" + name;
}

/* Relative to pickerRoot when listing is under it; otherwise absolute. */
function insertPath(root, listing, name) {
    if (name === "..") {
        return joinPath(listing, name);
    }
    if (!root || listing === root) {
        return name;
    }
    var abs = joinPath(listing, name);
    var prefix = root === "/" ? "/" : root + "/";
    if (abs.indexOf(prefix) === 0) {
        return abs.substring(prefix.length);
    }
    return abs;
}

function openFilePicker() {
    prepareOverlay("file-picker");
    var f = byId("f-text");
    if (f && document.activeElement === f) {
        f.blur();
    }
    clearMods();
    var search = byId("f-file-search");
    if (search) {
        search.value = "";
    }
    pickerRoot = "";
    lastPickerCwd = "";
    pickerAwaitingRoot = true;
    filePickerOpen = true;
    var btn = byId("btn-files");
    if (btn) {
        btn.className = "active";
    }
    var picker = byId("file-picker");
    if (picker) {
        picker.style.display = "";
    }
    var cwdEl = byId("file-cwd");
    if (cwdEl) {
        cwdEl.innerHTML = "Listing\u2026";
    }
    var list = byId("file-list");
    if (list) {
        list.innerHTML = '<div class="empty">Listing\u2026</div>';
    }
    sendCmd({ op: "list_files" });
    setTimeout(function () {
        var fs = byId("f-file-search");
        if (filePickerOpen && fs && document.activeElement !== fs) {
            fs.focus();
        }
    }, 0);
}

function closeFilePicker() {
    filePickerOpen = false;
    pickerRoot = "";
    lastPickerCwd = "";
    pickerAwaitingRoot = false;
    var picker = byId("file-picker");
    if (picker) {
        picker.style.display = "none";
    }
    var btn = byId("btn-files");
    if (btn) {
        btn.className = "";
    }
    var search = byId("f-file-search");
    if (search && document.activeElement === search) {
        search.blur();
    }
    holdVkb();
}

function toggleFilePicker() {
    if (filePickerOpen) {
        closeFilePicker();
    } else {
        openFilePicker();
    }
}

function renderFilePicker(data) {
    var list = byId("file-list");
    var cwdEl = byId("file-cwd");
    if (!list || !filePickerOpen) {
        return;
    }
    var cwd = (data && data.cwd) || "";
    if (cwd && cwd !== lastPickerCwd) {
        var navigated = lastPickerCwd !== "";
        lastPickerCwd = cwd;
        if (pickerAwaitingRoot || !pickerRoot) {
            pickerRoot = cwd;
            pickerAwaitingRoot = false;
        }
        var searchBox = byId("f-file-search");
        if (searchBox && navigated) {
            searchBox.value = "";
        }
    }
    if (cwdEl) {
        cwdEl.innerHTML = esc(cwd || lastPickerCwd || "Listing\u2026");
    }
    var raw = (byId("f-file-search").value || "").replace(/^\s+|\s+$/g, "");
    var q = raw.toLowerCase();
    var files = (data && data.files) || [];
    var shown = [];
    var i;
    for (i = 0; i < files.length; i++) {
        var name = files[i].name || "";
        if (!name) {
            continue;
        }
        if (name === ".." && cwd === "/") {
            continue;
        }
        var text = name.toLowerCase();
        var s = q ? fuzzyScore(q, text) : 1;
        if (s) {
            shown.push({ score: s, item: files[i], order: i });
        }
    }
    if (q) {
        shown.sort(function (a, b) {
            if (a.score !== b.score) {
                return b.score - a.score;
            }
            return a.order - b.order;
        });
    }
    fileRows = [];
    var html = "";
    for (i = 0; i < shown.length; i++) {
        var it = shown[i].item;
        fileRows.push(it);
        var ico = it.dir ? ICON_FOLDER : ICON_FILE;
        var label = it.dir ? (it.name + "/") : it.name;
        html += '<div class="win-menu-item" data-kind="' +
            (it.dir ? "dir" : "file") + '" data-name="' +
            esc(it.name).replace(/'/g, "&#39;") + '">' +
            '<span class="win-ico">' + ico + "</span>" +
            esc(label) + "</div>";
    }
    if ((data && data.filesTruncated) && !q) {
        html += '<div class="empty">Listing truncated \u2014 type to search</div>';
    }
    if (!fileRows.length && html === "") {
        html = '<div class="empty">' +
            (q ? "No matches" : (cwd ? "Empty directory" : "Listing\u2026")) +
            "</div>";
    }
    list.innerHTML = html;
}

function pickFileResult() {
    if (!fileRows.length) {
        return;
    }
    applyFileRow(fileRows[0]);
}

function applyFileRow(it) {
    if (!it || !it.name) {
        return;
    }
    var listing = lastPickerCwd || pickerRoot || "";
    if (it.dir) {
        var next = joinPath(listing, it.name);
        sendCmd({ op: "list_files", path: next });
        return;
    }
    insertPickedFile(insertPath(pickerRoot, listing, it.name));
}

function insertPickedFile(path) {
    if (!path) {
        return;
    }
    closeFilePicker();
    sendCmd({ op: "text", text: shellQuote(path), paste: true });
    keepCaptureFocus();
}

function showView(name) {
    prepareOverlay(name === "settings" ? "view-settings" : "");
    var settingsEl = byId("view-settings");
    var settingsOn = name === "settings";
    if (settingsOn) {
        clearMods();
    } else {
        holdVkb();
    }
    currentView = name;
    /* The terminal view stays visible behind the settings modal. */
    byId("view-main").style.display = "";
    if (settingsEl) {
        settingsEl.style.display = settingsOn ? "" : "none";
    }
    var gear = byId("tab-settings");
    if (gear) { gear.className = "tab" + (settingsOn ? " active" : ""); }
}

function startAutoRefresh() {
    if (refreshTimer) {
        clearTimeout(refreshTimer);
        refreshTimer = null;
    }
    var lastListAt = 0;
    function tick() {
        fetchStatusJson();
        /* list-windows is a blocking curl POST on the daemon's input
           thread — skip it while typing so a key is not queued behind it. */
        if (lastData && lastData.configured && Date.now() - lastInputAt > 2500 &&
                Date.now() - lastListAt > 2500) {
            lastListAt = Date.now();
            sendCmd({ op: "list" });
        }
        /* Copy mode overlay is recaptured in the daemon; poll often enough
           that cursor/scroll shows up on the e-ink without waiting 2.5s. */
        // Faster local status-file reads for pushed Herdr metadata. This is
        // still LIPC/status.json, not a browser network subscription.
        var ms = (lastData && lastData.eventUpdates) ? 500 :
            ((lastData && lastData.copyMode) ? 700 : 2500);
        refreshTimer = setTimeout(tick, ms);
    }
    tick();
}

/* Terminal view: tap the screen to bring up the on-screen keyboard and
   type straight into tmux. The f-text field is the keyboard's capture
   target — each keystroke is forwarded to the daemon (send-keys -l) and
   the field cleared, so the VKB acts as a raw keypad. */
function focusTerminalKeyboard() {
    if (composeVisible()) { return; }
    var f = byId("f-text");
    if (f) {
        f.value = "";
        /* blur+focus: focus() on an already-focused field is a no-op on
           this WebKit, so a second tap re-summons the VKB if it dropped */
        if (document.activeElement === f) {
            f.blur();
        }
        f.focus();
    }
    kbWanted = true;
}

function composeVisible() {
    var pop = byId("term-compose");
    return !!(pop && pop.style.display !== "none");
}
function hideCompose() {
    composeGeneration += 1;
    var pop = byId("term-compose");
    var field = byId("f-compose");
    if (field && document.activeElement === field) { field.blur(); }
    if (pop) { pop.style.display = "none"; }
}
function cancelCompose() {
    hideCompose();
    var field = byId("f-compose");
    if (field) { field.value = ""; }
    lastTermTapAt = 0;
    keepCaptureFocus();
}
function showCompose(text) {
    var pop = byId("term-compose");
    var field = byId("f-compose");
    if (!pop || !field) { return; }
    showView("view");
    kbWanted = false;
    pop.style.display = "";
    var generation = ++composeGeneration;
    field.value = (typeof text === "string") ? text : "";
    setTimeout(function () {
        if (composeVisible() && generation === composeGeneration) { field.focus(); }
    }, 0);
}
function sendCompose(withEnter) {
    var field = byId("f-compose");
    var text;
    if (!composeVisible() || composeSending) { return; }
    composeSending = true;
    text = field ? field.value : "";
    /* Consume before sending: this WebKit can deliver two mousedowns for one
       visible tap, and each press-flash callback otherwise sees the same text. */
    if (field) { field.value = ""; }
    if (text) {
        sendCmd({ op: "clipboard_remember", text: text });
        /* paste:true wraps CSI 200~/201~ so fish does not autopair quotes. */
        sendCmd({ op: "text", text: text, paste: true });
    }
    if (withEnter) { sendCmd({ op: "key", key: "Enter" }); }
    hideCompose();
    keepCaptureFocus();
    setTimeout(function () { composeSending = false; }, 500);
}

function wireCompose() {
    byId("btn-compose-send").addEventListener("mousedown", function (ev) {
        if (ev.preventDefault) { ev.preventDefault(); }
        var generation = composeGeneration;
        afterPressFlash(function () {
            if (generation === composeGeneration) { sendCompose(false); }
        });
    });
    byId("btn-compose-cancel").addEventListener("mousedown", function (ev) {
        if (ev.preventDefault) { ev.preventDefault(); }
        if (ev.stopPropagation) { ev.stopPropagation(); }
        cancelCompose();
    });
    byId("f-compose").addEventListener("keydown", function (ev) {
        var key = ev.keyCode || ev.which;
        if (key === 13 && ev.shiftKey) {
            if (ev.stopPropagation) { ev.stopPropagation(); }
            return true; /* Keep the textarea's native newline editing. */
        }
        if (key === 13 || key === 27) {
            if (ev.preventDefault) { ev.preventDefault(); }
            if (ev.stopPropagation) { ev.stopPropagation(); }
            if (key === 27) { cancelCompose(); } else { sendCompose(true); }
            return false;
        }
        return true;
    });
    document.addEventListener("mousedown", function (ev) {
        if (!composeVisible()) { return; }
        var node = ev.target || ev.srcElement;
        while (node && node !== document) {
            if (node.id === "term-compose") { return; }
            node = node.parentNode;
        }
        cancelCompose();
    });
}

function syncDaemonScroll(data) {
    if (data.copyMode || dragActive || typeof data.scrollOffset !== "number") { return; }
    var revision = data.scrollRevision || 0;
    var dOff = data.scrollOffset;
    if (revision < lastScrollRevision) { return; } // an older file poll
    if (revision > lastScrollRevision && !pendingRestore && !switchPending) {
        // Button actions are authoritative in both directions, including Live.
        lastScrollRevision = revision;
        lastDaemonOffset = dOff;
        viewOffset = dOff;
        scrollAckUntil = 0;
        return;
    }
    var awaiting = Date.now() < scrollAckUntil;
    if (awaiting && dOff === lastDaemonOffset) {
        scrollAckUntil = 0;
        awaiting = false;
    }
    // Keep the old stale-poll protection for locally driven touch scrolling.
    if (!awaiting && !pendingRestore && !switchPending && dOff > viewOffset) {
        lastDaemonOffset = dOff;
        viewOffset = dOff;
    }
}
/* Jump the WAF scrollback view to an exact offset (clamped). Used by
   drag-to-page, arrows, and search; the pane itself is never touched. */
function scrollToOffset(offset) {
    viewOffset = Math.max(0, offset);
    lastDaemonOffset = viewOffset;
    scrollAckUntil = Date.now() + 1500;
    if (lastPaneKey && lastSbLen > 0) {
        paneOffsets[lastPaneKey] = { off: viewOffset, sb: lastSbLen };
    }
    sendCmd({ op: "scroll_to", offset: viewOffset });
    queueRender();
}

function wireTerminalDrag() {
    var term = byId("term");
    if (!term) {
        return;
    }
    var dragStartY = null;
    term.addEventListener("touchstart", function (ev) {
        if (ev.touches && ev.touches.length) {
            dragStartY = ev.touches[0].clientY || 0;
        }
        dragActive = false;
        dragMoved = false;
    });
    term.addEventListener("mousedown", function (ev) {
        if (composeVisible()) {
            cancelCompose();
            if (ev.stopPropagation) { ev.stopPropagation(); }
            return;
        }
        var now = Date.now();
        if (now - lastTermTapAt < 450) {
            lastTermTapAt = 0;
            showCompose();
            if (ev.stopPropagation) { ev.stopPropagation(); }
            return;
        }
        lastTermTapAt = now;
        dragStartY = ev.clientY || 0;
        dragActive = false;
        dragMoved = false;
        /* NO preventDefault here: on this old WebKit it suppresses the
           click that follows a plain tap, so the keyboard would never
           open. The drag is handled by the mousemove listener. */
        /* The framework delivers ONLY the mousedown (no mouseup/click).
           Focusing during the mousedown itself is swallowed by the WebKit,
           so defer to the next tick; a real drag's movement blurs it. */
        setTimeout(focusTerminalKeyboard, 0);
    });
    document.addEventListener("mousemove", function (ev) {
        var pp = byId("paste-pop");
        if (pp && pp.style.display !== "none") {
            dragStartY = null;
            return;
        }
        if (dragStartY === null) {
            return;
        }
        var dy = dragStartY - (ev.clientY || 0);
        /* One swipe = one screen, same as the arrows / page-turn buttons.
           This WebKit often never sends mouseup, so disarm dragStartY after
           the jump; the next mousedown starts a new page. */
        if (Math.abs(dy) >= 12) {
            hidePastePop();
            hideSnippetPop();
            if (lastData && lastData.copyMode) {
                sendCmd({ op: "key", key: dy < 0 ? "ScrollUp" : "ScrollDown" });
                dragStartY = null;
                dragActive = true;
                dragEndAt = Date.now() + 700;
                dragMoved = true;
                return;
            }
            var page = (lastData && lastData.rows) ? lastData.rows : 24;
            var max = (lastData && lastData.scrollback) ? lastData.scrollback.length : 0;
            /* Finger down -> older history; finger up -> toward live. */
            var next = dy < 0 ?
                Math.min(max, viewOffset + page) :
                Math.max(0, viewOffset - page);
            scrollToOffset(next);
            dragStartY = null;
            dragActive = true;
            dragEndAt = Date.now() + 700;
            if (ev.preventDefault) {
                ev.preventDefault();
            }
        }
    });
    document.addEventListener("mouseup", function () {
        dragStartY = null;
        dragActive = false;
        /* settle: draw the final offset immediately, don't wait for the
           throttled timer or the next poll */
        if (renderTimer) {
            clearTimeout(renderTimer);
            renderTimer = null;
        }
        if (lastData) {
            safeRender(lastData);
        }
    });
}

function flushCaptureInput() {
    var f = byId("f-text");
    var v = f.value;
    if (!v) {
        return;
    }
    var key = CAPTURE_KEYS[v];
    if (key) {
        sendWithMods("key", key);
    } else {
        sendWithMods("text", v);
    }
    f.value = "";
}

function wireTerminalKeyboard() {
    var f = byId("f-text");
    if (!f) {
        return;
    }
    /* keyup and input both flush; the value is cleared after the first so
       the second event never double-sends. */
    f.addEventListener("keyup", flushCaptureInput);
    f.addEventListener("input", flushCaptureInput);
    /* Enter and Backspace always go to tmux; the capture field is empty
       between flushes. The VKB stays up. */
    f.addEventListener("keydown", function (ev) {
        var k = ev.keyCode || ev.which;
        if (k === 13) {
            sendWithMods("key", "Enter");
            this.value = "";
            if (ev.preventDefault) { ev.preventDefault(); }
            return false;
        }
        if (k === 8) {
            sendWithMods("key", "Backspace");
            if (ev.preventDefault) { ev.preventDefault(); }
            return false;
        }
        return true;
    });
    f.addEventListener("blur", function () {
        setTimeout(holdVkb, 50);
    });
}

document.addEventListener("DOMContentLoaded", function () {
    hookChromeOnGo(KMUX_APP, "KMux");
    paintFullGrid([], 80, 24);

    sendCmd({ op: "watch_start" });

    wireKbChrome();
    wireOverlayChrome();
    wireMenuGroups();
    fetchPromptRules();
    fetchQuickSnippets();
    holdVkb();
    window.addEventListener("resize", holdVkb);
    document.addEventListener("mousedown", function (ev) {
        flashPressed(closestButton(ev.target || ev.srcElement));
    });
    /* Tap the terminal screen: keyboard up, typing goes straight to tmux —
       unless the tap was the tail of a drag-to-scroll gesture. */
    byId("term").addEventListener("click", function () {
        if (dragActive && Date.now() < dragEndAt) {
            dragActive = false;
            return; /* a drag just ended — don't pop the keyboard */
        }
        dragActive = false;
        focusTerminalKeyboard();
    });
    wireTerminalKeyboard();
    wireTerminalDrag();

    /* Oasis physical page-turn buttons: when they reach the WebKit as
       PageUp/PageDown, scroll the remote pane (copy mode) instead of
       paging the page. */
    document.addEventListener("keydown", function (ev) {
        var k = ev.keyCode || ev.which;
        if (k === 33) {
            scrollUp();
            if (ev.preventDefault) { ev.preventDefault(); }
            return false;
        }
        if (k === 34) {
            scrollDown();
            if (ev.preventDefault) { ev.preventDefault(); }
            return false;
        }
        return true;
    });
    var settingsButton = byId("tab-settings");
    if (settingsButton) {
        settingsButton.addEventListener("mousedown", function () {
            var next = currentView === "settings" ? "view" : "settings";
            afterPressFlash(function () { showView(next); });
        });
    }
    /* Tapping the status chip toggles the error popup under it. */
    byId("state").addEventListener("mousedown", function (ev) {
        if (ev.preventDefault) {
            ev.preventDefault();
        }
        toggleErrPop();
    });

    /* Delegated taps on the window dropdown rows: walk up from the tap
       target so a tap on the row's text or padding still picks it. */
    byId("win-menu").addEventListener("mousedown", function (ev) {
        var node = ev.target || ev.srcElement;
        while (node && node !== this) {
            var kind = node.getAttribute && node.getAttribute("data-kind");
            var id = node.getAttribute && node.getAttribute("data-id");
            if (kind === "create-session" && id) {
                createSessionByName(id);
                return;
            }
            if (kind === "host" && id) {
                jumpToHost(id);
                return;
            }
            if (kind === "host-session" && id) {
                jumpToHostSession(node.getAttribute("data-host"), id);
                return;
            }
            if (kind === "agent" && id) {
                jumpToAgent(node.getAttribute("data-host"), node.getAttribute("data-session"),
                    node.getAttribute("data-tab"), id);
                return;
            }
            if (kind === "pane" && id) {
                jumpToPane(node.getAttribute("data-host"), node.getAttribute("data-session"),
                    node.getAttribute("data-tab"), id);
                return;
            }
            if (kind === "window" && id) {
                jumpToWindow(node.getAttribute("data-session"), id);
                return;
            }
            node = node.parentNode;
        }
    });
    /* Tapping anywhere on the borderless search row (the icon included)
       focuses the box; a focus inside a mousedown is swallowed by this
       WebKit, so defer it. */
    byId("menu-search-field").addEventListener("mousedown", function (ev) {
        var input = byId("f-menu-search");
        var t = ev.target || ev.srcElement;
        if (input && t !== input) {
            setTimeout(function () {
                if (menuOpen && input && document.activeElement !== input) {
                    input.focus();
                }
            }, 0);
        }
    });
    /* Dropdown search: re-filter as the user types; Enter picks a window
       (the active one when it is in the results, else the first result)
       and closes the dropdown. */
    byId("f-menu-search").addEventListener("keydown", function (ev) {
        var k = ev.keyCode || ev.which;
        if (k === 13) {
            pickMenuResult();
            if (ev.preventDefault) {
                ev.preventDefault();
            }
            return false;
        }
        return true;
    });
    byId("f-menu-search").addEventListener("keyup", function (ev) {
        var k = ev.keyCode || ev.which;
        if (k === 13) {
            pickMenuResult();
            if (ev.preventDefault) {
                ev.preventDefault();
            }
            return false;
        }
        if (lastData) {
            renderWindowMenu(lastData);
        }
        return true;
    });
    /* Terminal window tabs: tap to switch windows; "+" creates a window;
       the ellipsis opens Close/Next/Prev/Detach. These (and the session
       chip) close the other dropdown — never leave the user on a list. */
    byId("window-tabs").addEventListener("mousedown", function (ev) {
        var node = ev.target || ev.srcElement;
        while (node && node !== this) {
            var kind = node.getAttribute && node.getAttribute("data-kind");
            var id = node.getAttribute && node.getAttribute("data-id");
            if (kind === "session-chip") {
                /* Invert first: toggling .active in this tap would make
                   flashPressed restore the resting colors (no visible flash). */
                afterPressFlash(function () {
                    if (currentView === "settings") {
                        showView("view");
                    }
                    toggleWinMenu();
                });
                return;
            }
            if (kind === "wintab" && id) {
                beginPaneSwitch();
                sendCmd({ op: "window", index: id });
                showView("view");
                return;
            }
            if (kind === "wintab-new") {
                beginPaneSwitch();
                sendCmd({ op: "new_window" });
                showView("view");
                return;
            }
            if (kind === "wintab-more") {
                afterPressFlash(function () {
                    if (currentView === "settings") {
                        showView("view");
                    }
                    toggleActMenu();
                });
                return;
            }
            node = node.parentNode;
        }
    });
    byId("act-menu").addEventListener("mousedown", function (ev) {
        var node = ev.target || ev.srcElement;
        while (node && node !== this) {
            var op = node.getAttribute && node.getAttribute("data-op");
            if (op) {
                runWindowOp(op);
                return;
            }
            node = node.parentNode;
        }
    });
    /* Tapping anywhere outside a dropdown (or the chip/ellipsis that
       toggles it) closes it; the terminal tap then proceeds to raise
       the keyboard. */
    document.addEventListener("mousedown", function (ev) {
        if (!menuOpen && !actMenuOpen) {
            return;
        }
        var node = ev.target || ev.srcElement;
        while (node && node !== document) {
            if (node.id === "session-chip" || node.id === "win-menu" ||
                    node.id === "act-more" || node.id === "act-menu") {
                return;
            }
            node = node.parentNode;
        }
        closeWinMenu();
        closeActMenu();
    });
    byId("file-picker").addEventListener("mousedown", function (ev) {
        var node = ev.target || ev.srcElement;
        while (node && node !== this) {
            var kind = node.getAttribute && node.getAttribute("data-kind");
            var name = node.getAttribute && node.getAttribute("data-name");
            if ((kind === "file" || kind === "dir") && name) {
                applyFileRow({ name: name, dir: kind === "dir" });
                return;
            }
            node = node.parentNode;
        }
    });
    byId("file-search-field").addEventListener("mousedown", function (ev) {
        var input = byId("f-file-search");
        var t = ev.target || ev.srcElement;
        if (input && t !== input) {
            setTimeout(function () {
                if (filePickerOpen && input && document.activeElement !== input) {
                    input.focus();
                }
            }, 0);
        }
    });
    byId("f-file-search").addEventListener("keydown", function (ev) {
        var k = ev.keyCode || ev.which;
        if (k === 13) {
            pickFileResult();
            if (ev.preventDefault) {
                ev.preventDefault();
            }
            return false;
        }
        return true;
    });
    byId("f-file-search").addEventListener("keyup", function (ev) {
        var k = ev.keyCode || ev.which;
        if (k === 13) {
            pickFileResult();
            if (ev.preventDefault) {
                ev.preventDefault();
            }
            return false;
        }
        if (lastData) {
            renderFilePicker(lastData);
        }
        return true;
    });
    document.addEventListener("mousedown", function (ev) {
        if (!filePickerOpen) {
            return;
        }
        var node = ev.target || ev.srcElement;
        while (node && node !== document) {
            if (node.id === "file-picker" || node.id === "btn-files") {
                return;
            }
            node = node.parentNode;
        }
        closeFilePicker();
    });
    /* Tapping anywhere outside the settings modal closes it (gear toggles
       it; the chip/menu interactions close it through their own paths). */
    document.addEventListener("mousedown", function (ev) {
        if (currentView !== "settings") {
            return;
        }
        var node = ev.target || ev.srcElement;
        while (node && node !== document) {
            if (node.id === "view-settings" || node.id === "tab-settings" ||
                    node.id === "win-menu" || node.id === "session-chip" ||
                    node.id === "act-more" || node.id === "act-menu" ||
                    node.id === "file-picker" || node.id === "btn-files") {
                return;
            }
            node = node.parentNode;
        }
        showView("view");
    });
    var pasteList = byId("paste-list");
    if (pasteList) {
        pasteList.addEventListener("mousedown", function (ev) {
            var node = ev.target || ev.srcElement;
            var index;
            if (ev.stopPropagation) {
                ev.stopPropagation();
            }
            while (node && node !== pasteList) {
                index = node.getAttribute && node.getAttribute("data-clip-index");
                if (index !== null && typeof index !== "undefined") {
                    afterPressFlash(function () { loadClipboardHistory(parseInt(index, 10)); });
                    return;
                }
                node = node.parentNode;
            }
        });
    }
    var snippetList = byId("snippet-list");
    if (snippetList) {
        snippetList.addEventListener("mousedown", function (ev) {
            var node = ev.target || ev.srcElement;
            var index;
            if (ev.stopPropagation) {
                ev.stopPropagation();
            }
            while (node && node !== snippetList) {
                index = node.getAttribute && node.getAttribute("data-snippet-index");
                if (index !== null && typeof index !== "undefined") {
                    afterPressFlash(function () { loadQuickSnippet(parseInt(index, 10)); });
                    return;
                }
                node = node.parentNode;
            }
        });
    }
    document.addEventListener("mousedown", function (ev) {
        var pop = byId("paste-pop");
        if (!pop || pop.style.display === "none") {
            return;
        }
        var node = ev.target || ev.srcElement;
        while (node && node !== document) {
            if (node.id === "paste-pop") {
                return;
            }
            node = node.parentNode;
        }
        hidePastePop();
    });
    document.addEventListener("mousedown", function (ev) {
        var pop = byId("snippet-pop");
        if (!pop || pop.style.display === "none") {
            return;
        }
        var node = ev.target || ev.srcElement;
        while (node && node !== document) {
            if (node.id === "snippet-pop") {
                return;
            }
            node = node.parentNode;
        }
        hideSnippetPop();
    });
    /* Tapping anywhere except the chip or the popup closes the popup. */
    document.addEventListener("mousedown", function (ev) {
        var pop = byId("err-pop");
        if (!pop || pop.style.display === "none") {
            return;
        }
        var node = ev.target || ev.srcElement;
        while (node && node !== document) {
            if (node.id === "state" || node.id === "err-pop") {
                return;
            }
            node = node.parentNode;
        }
        hideErrPop();
    });

    byId("btn-diagnose").addEventListener("mousedown", function (ev) {
        if (ev.preventDefault) { ev.preventDefault(); }
        if (ev.stopPropagation) { ev.stopPropagation(); }
        paintDiagnose({ running: true, proxy: { ok: true, detail: "checking…" }, hosts: [] });
        sendCmd({ op: "diagnose" });
    });
    byId("btn-save").addEventListener("mousedown", function () {
        sendCmd({
            op: "config_set",
            proxy_url: byId("f-proxy").value.replace(/^\s+|\s+$/g, ""),
            token: byId("f-token").value.replace(/^\s+|\s+$/g, ""),
            socks5: byId("f-socks5").value.replace(/^\s+|\s+$/g, "")
        });
        byId("f-token").value = "";
        settingsDirty = false;
        showView("view");
        sendCmd({ op: "watch_start" });
    });
    ["f-proxy", "f-token", "f-socks5"].forEach(function (id) {
        byId(id).addEventListener("input", function () { settingsDirty = true; });
    });
    byId("btn-exit").addEventListener("mousedown", function () {
        afterPressFlash(function () {
            var kindle = getKindle();
            if (kindle && kindle.appmgr && typeof kindle.appmgr.back === "function") {
                kindle.appmgr.back(); /* exits the WAF back to the home screen */
            } else {
                setError("Exit unavailable (appmgr API missing)");
            }
        });
    });
    wireCompose();

    showPanel("scroll");

    fetchStatusJson();
    startAutoRefresh();
});
