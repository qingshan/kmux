/*
 * Synthetic touch and keyboard input for the Kindle, used by device e2e runs.
 *
 * The Kindle's X server has the XTEST extension built in, so XTestFake* events
 * reach the WAF exactly like the touchscreen and the system keyboard do. This is
 * the only way a script can open a popup, type into a field, or press a toolbar
 * button: the LIPC `cmd` channel drives the daemon, not the WAF's UI.
 *
 * Commands arrive one per line on stdin, so a caller can run a whole gesture
 * sequence in a single ssh round trip:
 *
 *   move X Y                 move the pointer
 *   tap X Y                  move, press, release
 *   hold X Y MS              move, press, release after MS
 *   drag X1 Y1 X2 Y2 [MS]    press, slide in steps, release
 *   key NAME                 press and release a keysym by name (e.g. Return)
 *   keydown NAME             press and hold, until `keyup`
 *   keyup NAME
 *   text STRING              type ASCII, spaces, and U+XXXX escapes
 *   sleep MS
 *
 * Coordinates are Kindle framebuffer pixels, which equal X screen pixels.
 * Blank lines and lines starting with # are ignored. Exit status is nonzero if
 * any command failed, so a caller can tell a dropped tap from a clean run.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

typedef struct _XDisplay Display;
typedef unsigned long KeySym;
typedef unsigned char KeyCode;

extern Display *XOpenDisplay(const char *);
extern int XDisplayWidth(Display *, int);
extern int XDisplayHeight(Display *, int);
extern int XTestFakeMotionEvent(Display *, int, int, int, unsigned long);
extern int XTestFakeButtonEvent(Display *, unsigned int, int, unsigned long);
extern int XTestFakeKeyEvent(Display *, unsigned int, int, unsigned long);
extern int XFlush(Display *);
extern int XSync(Display *, int);
extern int XCloseDisplay(Display *);
extern KeySym XStringToKeysym(const char *);
extern KeyCode XKeysymToKeycode(Display *, KeySym);
extern KeySym *XGetKeyboardMapping(Display *, KeyCode, int, int *);
extern int XChangeKeyboardMapping(Display *, int, int, KeySym *, int);
extern int XFree(void *);

#define LINE_MAX 4096
#define PRESS_MS 90
#define STEP_MS 30
#define DRAG_STEPS 12

static Display *dpy;
static int failures;
static KeyCode spare_code;
static KeySym spare_saved[2];
static int spare_saved_count;

static void move_to(int x, int y)
{
    XTestFakeMotionEvent(dpy, 0, x, y, 0);
    XFlush(dpy);
    XSync(dpy, 0);
}

static void sleep_ms(int ms)
{
    if (ms > 0)
        usleep((useconds_t)ms * 1000);
}

static void tap_at(int x, int y, int hold_ms)
{
    move_to(x, y);
    sleep_ms(STEP_MS);
    XTestFakeButtonEvent(dpy, 1, 1, 0);
    XFlush(dpy);
    XSync(dpy, 0);
    sleep_ms(hold_ms);
    XTestFakeButtonEvent(dpy, 1, 0, 0);
    XFlush(dpy);
    XSync(dpy, 0);
}

/* Shift level of a keysym on a keycode: 0 unshifted, 1 shifted, -1 absent. */
static int shift_level(KeyCode code, KeySym want)
{
    int per = 0, i, level = -1;
    KeySym *map = XGetKeyboardMapping(dpy, code, 1, &per);
    if (!map)
        return -1;
    for (i = 0; i < per && i < 2; i++)
        if (map[i] == want)
            level = i;
    XFree(map);
    return level;
}

/*
 * Keysyms missing from the keymap (anything outside the keyboard layout, such
 * as emoji or box drawing) get parked on an unused keycode for one keystroke,
 * then the original mapping is restored.
 */
static KeyCode remap_for(KeySym keysym)
{
    static const KeyCode candidates[] = { 255, 254, 253, 252, 251, 250, 249, 248 };
    size_t i;
    int per = 0;

    if (spare_code) {
        KeySym one[1] = { keysym };
        XChangeKeyboardMapping(dpy, (int)spare_code, 1, one, 1);
        XSync(dpy, 0);
        return spare_code;
    }
    for (i = 0; i < sizeof(candidates) / sizeof(candidates[0]); i++) {
        KeySym *existing = XGetKeyboardMapping(dpy, candidates[i], 1, &per);
        int j;
        KeySym one[1];
        if (!existing)
            continue;
        spare_saved_count = per > 2 ? 2 : per;
        for (j = 0; j < spare_saved_count; j++)
            spare_saved[j] = existing[j];
        XFree(existing);
        spare_code = candidates[i];
        one[0] = keysym;
        XChangeKeyboardMapping(dpy, (int)spare_code, 1, one, 1);
        XSync(dpy, 0);
        return spare_code;
    }
    return 0;
}

static void restore_spare(void)
{
    KeySym restored[2];
    if (!spare_code || !spare_saved_count)
        return;
    restored[0] = spare_saved[0];
    restored[1] = spare_saved_count > 1 ? spare_saved[1] : spare_saved[0];
    XChangeKeyboardMapping(dpy, (int)spare_code, 2, restored, 2);
    XSync(dpy, 0);
    spare_code = 0;
}

static void press_keysym(KeySym keysym)
{
    KeyCode code = XKeysymToKeycode(dpy, keysym);
    KeyCode shift;
    int shifted = 0;

    if (code) {
        shifted = shift_level(code, keysym) == 1;
    } else {
        code = remap_for(keysym);
        if (!code) {
            fprintf(stderr, "kindle_xinput: cannot type keysym 0x%lx\n", keysym);
            failures++;
            return;
        }
    }
    shift = XKeysymToKeycode(dpy, XStringToKeysym("Shift_L"));
    if (shifted)
        XTestFakeKeyEvent(dpy, shift, 1, 0);
    XTestFakeKeyEvent(dpy, code, 1, 0);
    XFlush(dpy);
    XSync(dpy, 0);
    sleep_ms(STEP_MS);
    XTestFakeKeyEvent(dpy, code, 0, 0);
    if (shifted)
        XTestFakeKeyEvent(dpy, shift, 0, 0);
    XFlush(dpy);
    XSync(dpy, 0);
    sleep_ms(STEP_MS);
    restore_spare();
}

/* XStringToKeysym accepts letters and digits literally, but punctuation needs
 * its canonical X keysym name on the Kindle's compact keymap. */
static KeySym ascii_keysym(unsigned char ch)
{
    static const char *names[128] = {
        [' '] = "space", ['!'] = "exclam", ['\"'] = "quotedbl",
        ['#'] = "numbersign", ['$'] = "dollar", ['%'] = "percent",
        ['&'] = "ampersand", ['\''] = "apostrophe", ['('] = "parenleft",
        [')'] = "parenright", ['*'] = "asterisk", ['+'] = "plus",
        [','] = "comma", ['-'] = "minus", ['.'] = "period", ['/'] = "slash",
        [':'] = "colon", [';'] = "semicolon", ['<'] = "less", ['='] = "equal",
        ['>'] = "greater", ['?'] = "question", ['@'] = "at",
        ['['] = "bracketleft", ['\\'] = "backslash", [']'] = "bracketright",
        ['^'] = "asciicircum", ['_'] = "underscore", ['`'] = "grave",
        ['{'] = "braceleft", ['|'] = "bar", ['}'] = "braceright",
        ['~'] = "asciitilde"
    };
    char name[2];

    if (ch < sizeof(names) && names[ch])
        return XStringToKeysym(names[ch]);
    name[0] = (char)ch;
    name[1] = 0;
    return XStringToKeysym(name);
}

static void type_text(const unsigned char *text)
{
    while (*text) {
        KeySym keysym = 0;
        unsigned long codepoint;

        if (text[0] == 'U' && text[1] == '+') {
            codepoint = strtoul((const char *)text + 2, NULL, 16);
            keysym = 0x01000000UL | codepoint;
            while (*text && *text != ' ')
                text++;
        } else if (*text < 0x80) {
            keysym = ascii_keysym(*text++);
        } else {
            text++;
            continue;
        }
        if (!keysym) {
            fprintf(stderr, "kindle_xinput: no keysym for byte 0x%02x\n", text[-1]);
            failures++;
            continue;
        }
        press_keysym(keysym);
    }
}

static int numbers(char *rest, int *values, int max)
{
    char *tok;
    int count = 0;
    for (tok = strtok(rest, " \t"); tok && count < max; tok = strtok(NULL, " \t"))
        values[count++] = atoi(tok);
    return count;
}

int main(void)
{
    char line[LINE_MAX];

    dpy = XOpenDisplay(NULL);
    if (!dpy) {
        fprintf(stderr, "kindle_xinput: cannot open DISPLAY\n");
        return 2;
    }
    printf("display %dx%d\n", XDisplayWidth(dpy, 0), XDisplayHeight(dpy, 0));
    fflush(stdout);

    while (fgets(line, sizeof(line), stdin)) {
        char *cmd = line, *rest;
        size_t len = strlen(cmd);
        int values[5];
        int count;

        while (len && (cmd[len - 1] == '\n' || cmd[len - 1] == '\r'))
            cmd[--len] = 0;
        while (*cmd == ' ' || *cmd == '\t')
            cmd++;
        if (!*cmd || *cmd == '#')
            continue;
        rest = strpbrk(cmd, " \t");
        if (rest) {
            *rest++ = 0;
            while (*rest == ' ' || *rest == '\t')
                rest++;
        } else {
            rest = cmd + strlen(cmd);
        }

        if (!strcmp(cmd, "move") || !strcmp(cmd, "tap") || !strcmp(cmd, "hold")) {
            count = numbers(rest, values, 3);
            if (count < 2) {
                fprintf(stderr, "kindle_xinput: %s needs X Y\n", cmd);
                failures++;
            } else if (!strcmp(cmd, "move")) {
                move_to(values[0], values[1]);
            } else if (!strcmp(cmd, "tap")) {
                tap_at(values[0], values[1], PRESS_MS);
            } else {
                tap_at(values[0], values[1], count >= 3 ? values[2] : 1200);
            }
        } else if (!strcmp(cmd, "drag")) {
            count = numbers(rest, values, 5);
            if (count < 4) {
                fprintf(stderr, "kindle_xinput: drag needs X1 Y1 X2 Y2\n");
                failures++;
            } else {
                int total = count >= 5 ? values[4] : 400;
                int i;
                move_to(values[0], values[1]);
                sleep_ms(STEP_MS);
                XTestFakeButtonEvent(dpy, 1, 1, 0);
                XFlush(dpy);
                XSync(dpy, 0);
                for (i = 1; i <= DRAG_STEPS; i++) {
                    move_to(values[0] + (values[2] - values[0]) * i / DRAG_STEPS,
                            values[1] + (values[3] - values[1]) * i / DRAG_STEPS);
                    sleep_ms(total / DRAG_STEPS);
                }
                XTestFakeButtonEvent(dpy, 1, 0, 0);
                XFlush(dpy);
                XSync(dpy, 0);
            }
        } else if (!strcmp(cmd, "key") || !strcmp(cmd, "keydown") || !strcmp(cmd, "keyup")) {
            KeySym keysym = XStringToKeysym(rest);
            KeyCode code;
            int down = strcmp(cmd, "keyup") != 0;
            if (!keysym) {
                fprintf(stderr, "kindle_xinput: unknown keysym '%s'\n", rest);
                failures++;
                continue;
            }
            code = XKeysymToKeycode(dpy, keysym);
            if (!code)
                code = remap_for(keysym);
            if (!code) {
                failures++;
                continue;
            }
            XTestFakeKeyEvent(dpy, code, down, 0);
            XFlush(dpy);
            XSync(dpy, 0);
            if (!strcmp(cmd, "key")) {
                sleep_ms(STEP_MS);
                XTestFakeKeyEvent(dpy, code, 0, 0);
                XFlush(dpy);
                XSync(dpy, 0);
                restore_spare();
            }
            sleep_ms(STEP_MS);
        } else if (!strcmp(cmd, "text")) {
            type_text((const unsigned char *)rest);
        } else if (!strcmp(cmd, "sleep")) {
            sleep_ms(atoi(rest));
        } else {
            fprintf(stderr, "kindle_xinput: unknown command '%s'\n", cmd);
            failures++;
        }
    }
    restore_spare();
    XCloseDisplay(dpy);
    return failures ? 1 : 0;
}
