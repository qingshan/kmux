#!/bin/sh
# Shared Kindle-side helpers for qingshan kpm packages.
#
# Sourced (not executed) by install/launch/uninstall and the on-device
# scripts. Callers set PKG_ID / DAEMON / LIPC_ID / APP_ID / DEST /
# KEEP_ON_UPGRADE / UPSTART_NAME via pkg.env before calling these
# functions. POSIX sh (Kindle ash); no bash arrays.

pkg_remount_rw() {
    if command -v mntroot >/dev/null 2>&1; then
        mntroot rw
    else
        mount -o remount,rw /
    fi
}

pkg_remount_ro() {
    if command -v mntroot >/dev/null 2>&1; then
        mntroot ro
    else
        mount -o remount,ro /
    fi
}

pkg_daemon_up() {
    timeout 5 lipc-get-prop "$LIPC_ID" info >/dev/null 2>&1
}

pkg_copy_scriptlet() {
    echo "Copying $PKG_ID scriptlet..."
    if [ -f "/mnt/us/documents/${PKG_ID}.sh" ]; then
        rm -f "/mnt/us/documents/${PKG_ID}.sh"
        sleep 1
    fi
    cp -a "./scriptlets/${PKG_ID}.sh" "/mnt/us/documents/${PKG_ID}.sh"
}

# Portable needs-copy: print "yes" if any SRC file is missing or differs.
# Directories in SRC (e.g. kmux fonts/) are walked; daemon-written files
# that exist only in DEST (status.json) are left alone.
pkg_waf_needs_copy_yes() {
    _src=$1
    _dst=$2
    if [ ! -d "$_dst" ]; then
        echo yes
        return
    fi
    find "$_src" -type f | while read -r _f; do
        _rel=${_f#$_src/}
        if [ ! -f "$_dst/$_rel" ] || ! cmp -s "$_f" "$_dst/$_rel"; then
            echo yes
            break
        fi
    done
}

pkg_register_waf() {
    _waf_src=${WAF_SRC:-$DEST/waf}
    _waf_dst="/var/local/mesquite/${PKG_ID}"
    _appreg=/var/local/appreg.db

    if [ ! -d "$_waf_src" ]; then
        echo "error: $_waf_src is missing - reinstall the $PKG_ID package" >&2
        return 1
    fi

    _need=$(pkg_waf_needs_copy_yes "$_waf_src" "$_waf_dst")
    if [ -z "$_need" ]; then
        return 0
    fi

    mkdir -p "$_waf_dst"
    # Copy in place rather than rm -rf: a live Mesquite instance would
    # otherwise read deleted entries. status.json (daemon-written, not in
    # SRC) survives because cp does not delete extra files in DEST.
    cp -r "$_waf_src/." "$_waf_dst/"

    sqlite3 "$_appreg" <<EOF
INSERT OR IGNORE INTO interfaces(interface) VALUES('application');
INSERT OR IGNORE INTO handlerIds(handlerId) VALUES('$APP_ID');
INSERT OR REPLACE INTO properties(handlerId,name,value) VALUES('$APP_ID','lipcId','$APP_ID');
INSERT OR REPLACE INTO properties(handlerId,name,value) VALUES('$APP_ID','command','/usr/bin/mesquite -l $APP_ID -c file://$_waf_dst/');
INSERT OR REPLACE INTO properties(handlerId,name,value) VALUES('$APP_ID','supportedOrientation','U');
EOF
}

pkg_unregister_waf() {
    _appreg=/var/local/appreg.db
    if [ -f "$_appreg" ]; then
        sqlite3 "$_appreg" \
            "DELETE FROM properties WHERE handlerId='$APP_ID'; DELETE FROM handlerIds WHERE handlerId='$APP_ID';" \
            2>/dev/null || true
    fi
    rm -rf "/var/local/mesquite/${PKG_ID}"
}

pkg_start_daemon() {
    mkdir -p "$DEST/var"
    _log=$DEST/var/start_log.txt
    if pkg_daemon_up; then
        return 0
    fi
    if start "$UPSTART_NAME" 2>/dev/null; then
        sleep 1
        if pkg_daemon_up; then
            return 0
        fi
    fi
    echo "[$(date)] starting $DAEMON detached" >>"$_log"
    "$DEST/sbin/$DAEMON" >"$DEST/var/${DAEMON}.err" 2>&1
    sleep 1
    if ! pkg_daemon_up; then
        echo "[$(date)] $DAEMON failed to start - stderr:" >>"$_log"
        tail -n 15 "$DEST/var/${DAEMON}.err" 2>/dev/null >>"$_log"
        return 1
    fi
}

pkg_stop_daemon() {
    lipc-set-prop "$LIPC_ID" exit "stop" 2>/dev/null || true
    stop "$UPSTART_NAME" 2>/dev/null || true
    pkill "$DAEMON" 2>/dev/null || true
    sleep 1
}

pkg_upstart_body() {
    cat <<EOF
start on framework_ready
stop on stopping framework
respawn

script
    source /etc/upstart/functions
    f_log I $DAEMON start "" "starting $DAEMON"
    exec $DEST/sbin/$DAEMON -n
end script
EOF
}

# Write /etc/upstart/$UPSTART_NAME.conf, reload, stop+pkill, start, then
# fall back to a detached daemon if the LIPC service still isn't up.
pkg_install_upstart() {
    echo "Installing the $DAEMON upstart job..."
    _job=$(pkg_upstart_body)
    _conf=/etc/upstart/${UPSTART_NAME}.conf
    if ! printf '%s\n' "$_job" >"$_conf" 2>/dev/null; then
        pkg_remount_rw 2>/dev/null || true
        if ! printf '%s\n' "$_job" >"$_conf" 2>/dev/null; then
            echo "warning: cannot write $_conf (read-only /?) - starting $DAEMON without autostart" >&2
            nohup "$DEST/sbin/$DAEMON" -n >/dev/null 2>&1 &
        fi
        pkg_remount_ro 2>/dev/null || true
    fi
    initctl reload-configuration 2>/dev/null || true
    stop "$UPSTART_NAME" 2>/dev/null || true
    if [ "${PKG_PKILL_ON_INSTALL:-1}" = "1" ]; then
        pkill "$DAEMON" 2>/dev/null || true
        sleep 1
    fi
    start "$UPSTART_NAME" 2>/dev/null || true
    sleep 1
    pkg_ensure_daemon
}

pkg_ensure_daemon() {
    if pkg_daemon_up; then
        return 0
    fi
    # Avoid stacking a duplicate while the freshly started job is still
    # initialising: skip the fallback if a process is already there.
    if [ "${PKG_PKILL_ON_INSTALL:-1}" = "1" ]; then
        # [k]muxd-style self-grep: first char of DAEMON in a character class.
        _pat="[${DAEMON%${DAEMON#?}}]${DAEMON#?}"
        if ps | grep -q "$_pat"; then
            return 0
        fi
    fi
    echo "$DAEMON service not responding - starting detached daemon"
    "$DEST/sbin/$DAEMON" >"$DEST/var/${DAEMON}.err" 2>&1 || true
    sleep 1
    if ! pkg_daemon_up; then
        echo "warning: $DAEMON service still not responding - WAF commands will not work" >&2
        echo "$DAEMON stderr:" >&2
        tail -n 15 "$DEST/var/${DAEMON}.err" 2>/dev/null >&2
    fi
}

pkg_remove_upstart() {
    stop "$UPSTART_NAME" 2>/dev/null || true
    pkill "$DAEMON" 2>/dev/null || true
    pkg_remount_rw 2>/dev/null || true
    rm -f "/etc/upstart/${UPSTART_NAME}.conf"
    if [ -n "$EXTRA_UPSTART" ]; then
        for _j in $EXTRA_UPSTART; do
            stop "$_j" 2>/dev/null || true
            rm -f "/etc/upstart/${_j}.conf"
        done
    fi
    pkg_remount_ro 2>/dev/null || true
}

pkg_copy_lib_into_dest() {
    _lib=""
    if [ -n "${SCRIPT_DIR:-}" ] && [ -f "$SCRIPT_DIR/common/pkg-lib.sh" ]; then
        _lib=$SCRIPT_DIR/common/pkg-lib.sh
    elif [ -n "${SCRIPT_DIR:-}" ] && [ -f "$SCRIPT_DIR/../common/pkg-lib.sh" ]; then
        _lib=$SCRIPT_DIR/../common/pkg-lib.sh
    fi
    if [ -n "$_lib" ]; then
        cp -f "$_lib" "$DEST/scripts/pkg-lib.sh"
    fi
    if [ -n "${SCRIPT_DIR:-}" ] && [ -f "$SCRIPT_DIR/pkg.env" ]; then
        cp -f "$SCRIPT_DIR/pkg.env" "$DEST/scripts/pkg.env"
    fi
}

pkg_seed_if_missing() {
    _src=$1
    _dst=$2
    if [ ! -f "$_dst" ]; then
        echo "Seeding $(basename "$_dst")..."
        mkdir -p "$(dirname "$_dst")"
        cp "$_src" "$_dst"
    else
        echo "Keeping existing $(basename "$_dst") (not reset)"
    fi
}

pkg_install_files() {
    mkdir -p "$DEST/sbin" "$DEST/scripts" "$DEST/var"

    echo "Installing the $DAEMON daemon..."
    cp -f "./${PKG_ID}/sbin/${DAEMON}" "$DEST/sbin/${DAEMON}"
    chmod 755 "$DEST/sbin/${DAEMON}"

    echo "Installing $PKG_ID WAF..."
    rm -rf "$DEST/waf"
    cp -r ./waf "$DEST/waf"

    echo "Installing $PKG_ID scripts..."
    cp -f "./${PKG_ID}/scripts/"*.sh "$DEST/scripts/" 2>/dev/null || true
    pkg_copy_lib_into_dest
    chmod +x "$DEST/scripts/"*.sh 2>/dev/null || true

    pkg_copy_scriptlet
}

pkg_install() {
    pkg_install_files
    if [ -n "${SEED_SRC:-}" ] && [ -n "${SEED_DEST_REL:-}" ]; then
        pkg_seed_if_missing "$SEED_SRC" "$DEST/$SEED_DEST_REL"
    fi
    echo "Registering the $PKG_ID WAF..."
    if [ -x "$DEST/scripts/register-waf.sh" ]; then
        "$DEST/scripts/register-waf.sh" || echo "warning: WAF registration failed - is sqlite3 available? Tapping the Home screen icon will retry this." >&2
    fi
    pkg_install_upstart
}

pkg_keep_name() {
    _base=${1##*/}
    for _k in $KEEP_ON_UPGRADE; do
        if [ "$_base" = "$_k" ]; then
            return 0
        fi
    done
    return 1
}

pkg_uninstall() {
    UPGRADING=false
    [ "${1:-}" = "upgrade" ] && UPGRADING=true

    if [ -x "$DEST/scripts/stop.sh" ]; then
        echo "Stopping $PKG_ID..."
        "$DEST/scripts/stop.sh" || true
    fi
    if [ -x "$DEST/scripts/unregister-waf.sh" ]; then
        echo "Unregistering the $PKG_ID WAF..."
        "$DEST/scripts/unregister-waf.sh" || true
    fi

    echo "Stopping the $DAEMON LIPC daemon..."
    pkg_remove_upstart

    echo "Deleting scriptlet for $PKG_ID..."
    rm -f "/mnt/us/documents/${PKG_ID}.sh"

    if [ "$UPGRADING" = true ]; then
        echo "Upgrade - keeping $PKG_ID $KEEP_ON_UPGRADE"
        for _f in "$DEST/"*; do
            [ -e "$_f" ] || continue
            if pkg_keep_name "$_f"; then
                continue
            fi
            rm -rf "$_f"
        done
    else
        echo "Deleting $PKG_ID folder..."
        rm -rf "$DEST"
    fi
}

pkg_launch_lock() {
    export DISPLAY=:0
    mkdir -p "$DEST/var"
    LOCK=$DEST/var/launch.lock
    if ! mkdir "$LOCK" 2>/dev/null; then
        if [ -f "$LOCK/pid" ]; then
            _owner=$(cat "$LOCK/pid" 2>/dev/null)
            if [ -n "$_owner" ] && kill -0 "$_owner" 2>/dev/null; then
                return 1
            fi
        fi
        rm -rf "$LOCK" 2>/dev/null || true
        mkdir "$LOCK" 2>/dev/null || return 1
    fi
    echo $$ >"$LOCK/pid"
    trap 'rm -rf "$LOCK" 2>/dev/null || true' EXIT INT TERM
    return 0
}

pkg_kill_mesquite() {
    for _p in /proc/[0-9]*; do
        [ "$(cat "$_p/comm" 2>/dev/null)" = mesquite ] && kill "${_p#/proc/}" 2>/dev/null || true
    done
    sleep 1
}

pkg_appmgrd_start() {
    _log=$DEST/var/launch_log.txt
    _started=false
    _i=0
    while [ "$_i" -lt 3 ]; do
        if timeout 10 lipc-set-prop com.lab126.appmgrd start "app://${APP_ID}" 2>/dev/null; then
            _started=true
            break
        fi
        _i=$((_i + 1))
        sleep 1
    done
    if [ "$_started" = false ]; then
        echo "[$(date)] appmgrd start failed after 3 attempts - check /var/local/appreg.db registration" >>"$_log"
        return 1
    fi
}

pkg_launch() {
    if ! pkg_launch_lock; then
        exit 0
    fi
    _log=$DEST/var/launch_log.txt
    if ! pkg_daemon_up; then
        "$DEST/scripts/start.sh" >>"$_log" 2>&1 \
            || echo "[$(date)] start.sh failed - see $DEST/var/start_log.txt" >>"$_log"
    fi
    if ! "$DEST/scripts/register-waf.sh"; then
        echo "[$(date)] register-waf.sh failed - WAF may not launch" >>"$_log"
    fi
    pkg_kill_mesquite
    pkg_appmgrd_start
}
