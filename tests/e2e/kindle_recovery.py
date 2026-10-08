"""Read-only terminal recovery checks; run via kmux_e2e.py --kindle-recovery."""
from kmux_e2e import Artifacts, KindlePage
from kindle_lib import Device, ROOT, launch_waf


def main():
    device = Device()
    artifacts = Artifacts(ROOT / 'target/e2e/recovery')
    page = KindlePage(device, artifacts)
    prevent = device.ssh('lipc-get-prop com.lab126.powerd preventScreenSaver').stdout.strip()

    def live():
        return page.expect(lambda s: s.get('connected') and s.get('uiActive')
                           and not s.get('lastError'), 'connected foreground', timeout=40)

    try:
        page.prevent_sleep(True)
        live()
        # Several snapshots must preserve the local foreground state.
        for _ in range(3):
            before = live()['updatedAt']
            page.expect(lambda s: s.get('updatedAt', 0) > before and s.get('uiActive'),
                        'foreground survives next snapshot')
        with device.proxy_url(page.snapshot()['proxyUrl'].rstrip('/') + '/kmux-e2e-missing'):
            page.expect(lambda s: not s.get('connected') and 'HTTP 404' in (s.get('lastError') or ''),
                        'HTTP status is readable')
            page.screenshot('http-error')
        live()  # No watch_start or manual reconnect: background polling recovers.
        with device.proxy_network_outage():
            page.expect(lambda s: not s.get('connected') and 'proxy' in (s.get('lastError') or '').lower(),
                        'network failure is reported')
            page.screenshot('network-error')
        live()
        page.screenshot('network-recovered')
        page.prevent_sleep(False)
        device.sleep_screen()
        page.expect(lambda s: s.get('uiActive') is False, 'sleep pauses foreground polling')
        device.wake()
        live()
        page.prevent_sleep(True)
        # Home and reopen exercise Mesquite's native ongo callback.
        device.home()
        page.expect(lambda s: s.get('uiActive') is False, 'Home pauses foreground polling')
        launch_waf()
        live()
        page.screenshot('reopened')
        print('Kindle recovery: HTTP errors, network loss, sleep/wake, reopen, foreground OK')
    finally:
        try:
            device.wake()
        finally:
            try:
                page.prevent_sleep(prevent == '1')
            finally:
                artifacts.animation()
