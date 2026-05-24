// Klippo Clipboard Bridge — GNOME Shell extension (ESM, GNOME 45+).
//
// Why this exists: on GNOME Wayland the Mutter compositor implements neither
// wlr-data-control nor ext-data-control, so an external app cannot monitor the
// clipboard. Code running *inside* the Shell can, via Meta.Selection's
// owner-changed signal + St.Clipboard. This extension does only that: it
// watches the CLIPBOARD selection, debounces and de-duplicates, and forwards
// new text (and images, unless disabled) to the Klippo daemon over D-Bus.
//
// It has no UI and stores nothing — all history/search/config lives in the
// Rust daemon.

import Meta from 'gi://Meta';
import St from 'gi://St';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'org.klippo.Daemon';
const OBJECT_PATH = '/org/klippo/Daemon';
const CAPTURE_IFACE = 'org.klippo.Capture1';
const DAEMON_IFACE = 'org.klippo.Daemon1';

// Coalesce bursts of owner-changed signals (one copy can fire several).
const DEBOUNCE_MS = 200;
// Liveness ping + config refresh interval.
const HEARTBEAT_S = 5;

export default class KlippoBridgeExtension extends Extension {
    enable() {
        this._selection = global.display.get_selection();
        this._clipboard = St.Clipboard.get_default();
        this._lastText = null;
        this._lastImageSig = null;
        this._ignoreImages = true; // refreshed from the daemon below
        this._popupAtCursor = false; // refreshed from the daemon below
        this._pendingId = 0;

        this._ownerChangedId = this._selection.connect(
            'owner-changed',
            (_selection, selectionType, _source) => {
                // Only the CLIPBOARD selection (Ctrl+C). PRIMARY (mouse
                // selection) is ignored by default, matching Klipper.
                if (selectionType === Meta.SelectionType.SELECTION_CLIPBOARD)
                    this._scheduleCapture();
            }
        );

        this._refreshConfig();
        this._call(CAPTURE_IFACE, 'Heartbeat', null);
        this._heartbeatId = GLib.timeout_add_seconds(
            GLib.PRIORITY_DEFAULT,
            HEARTBEAT_S,
            () => {
                this._call(CAPTURE_IFACE, 'Heartbeat', null);
                this._refreshConfig();
                return GLib.SOURCE_CONTINUE;
            }
        );

        // Position the popup near the pointer when it opens. Wayland clients
        // cannot self-position, but the Shell can — watch new windows (and any
        // already open) and move the Klippo popup to the cursor on show.
        this._windowCreatedId = global.display.connect('window-created', (_d, win) =>
            this._onWindowCreated(win)
        );
        for (const actor of global.get_window_actors())
            this._onWindowCreated(actor.meta_window);
    }

    disable() {
        if (this._ownerChangedId) {
            this._selection?.disconnect(this._ownerChangedId);
            this._ownerChangedId = 0;
        }
        if (this._pendingId) {
            GLib.source_remove(this._pendingId);
            this._pendingId = 0;
        }
        if (this._heartbeatId) {
            GLib.source_remove(this._heartbeatId);
            this._heartbeatId = 0;
        }
        if (this._windowCreatedId) {
            global.display.disconnect(this._windowCreatedId);
            this._windowCreatedId = 0;
        }
        this._selection = null;
        this._clipboard = null;
        this._lastText = null;
    }

    _onWindowCreated(win) {
        try {
            if (!win)
                return;
            // On Wayland the title/app-id are still null at 'window-created' —
            // they arrive in later commits. Wait for the title, then identify
            // our popup by it ("Klippo"). The dialogs have other titles, so
            // they are left where the compositor placed them.
            let handled = false;
            const ids = [];
            const finish = () => {
                for (const id of ids) {
                    try {
                        win.disconnect(id);
                    } catch (_e) {}
                }
                ids.length = 0;
            };
            const tryMove = () => {
                if (handled || win.get_title() !== 'Klippo')
                    return;
                handled = true;
                finish();
                // Honour the daemon's popup_at_cursor setting; otherwise leave
                // the window where the compositor placed it.
                if (!this._popupAtCursor)
                    return;
                // Defer so the window has its final size before we move it.
                GLib.timeout_add(GLib.PRIORITY_DEFAULT, 40, () => {
                    this._moveToPointer(win);
                    return GLib.SOURCE_REMOVE;
                });
            };
            ids.push(win.connect('notify::title', tryMove));
            tryMove(); // in case the title is already set
            // Safety net if notify::title doesn't fire.
            GLib.timeout_add(GLib.PRIORITY_DEFAULT, 250, () => {
                tryMove();
                return GLib.SOURCE_REMOVE;
            });
        } catch (e) {
            console.error(`klippo: window-created handler error: ${e}`);
        }
    }

    _moveToPointer(win) {
        try {
            const [px, py] = global.get_pointer();
            const rect = win.get_frame_rect();
            const area = win.get_work_area_current_monitor();
            // Small offset from the cursor, clamped so the whole window stays in
            // the monitor work area.
            const x = Math.round(Math.max(area.x, Math.min(px + 6, area.x + area.width - rect.width)));
            const y = Math.round(Math.max(area.y, Math.min(py + 6, area.y + area.height - rect.height)));
            // `move_frame` was removed in recent Mutter; prefer move_resize_frame.
            if (typeof win.move_resize_frame === 'function')
                win.move_resize_frame(true, x, y, rect.width, rect.height);
            else if (typeof win.move_frame === 'function')
                win.move_frame(true, x, y);
            else {
                console.error('klippo: no window-move method available');
                return;
            }
        } catch (e) {
            console.error(`klippo: move failed: ${e}`);
        }
    }

    _scheduleCapture() {
        if (this._pendingId)
            GLib.source_remove(this._pendingId);
        this._pendingId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, DEBOUNCE_MS, () => {
            this._pendingId = 0;
            this._capture();
            return GLib.SOURCE_REMOVE;
        });
    }

    _capture() {
        if (!this._clipboard)
            return;
        this._clipboard.get_text(St.ClipboardType.CLIPBOARD, (_clipboard, text) => {
            if (text && text.length > 0) {
                // De-dupe: also collapses the echo of our own clipboard writes
                // when an item is selected from the popup.
                if (text === this._lastText)
                    return;
                this._lastText = text;
                this._lastImageSig = null;
                this._call(CAPTURE_IFACE, 'AddText', new GLib.Variant('(ss)', [text, 'clipboard']));
                return;
            }
            // No text → it may be an image or an actual clear. We still inspect
            // image/png when images are ignored so clears can be reported.
            this._clipboard.get_content(St.ClipboardType.CLIPBOARD, 'image/png', (_c, bytes) => {
                if (!bytes) {
                    this._lastText = null;
                    this._lastImageSig = null;
                    this._call(CAPTURE_IFACE, 'ClipboardCleared', new GLib.Variant('(s)', ['clipboard']));
                    return;
                }
                const data = bytes.get_data();
                if (!data || data.length === 0) {
                    this._lastText = null;
                    this._lastImageSig = null;
                    this._call(CAPTURE_IFACE, 'ClipboardCleared', new GLib.Variant('(s)', ['clipboard']));
                    return;
                }
                this._lastText = null;
                if (this._ignoreImages)
                    return;
                const sig = this._hashBytes(data);
                if (sig === this._lastImageSig)
                    return;
                this._lastImageSig = sig;
                const ay = GLib.Variant.new_from_bytes(new GLib.VariantType('ay'), bytes, true);
                const params = GLib.Variant.new_tuple([
                    GLib.Variant.new_string('image/png'),
                    ay,
                    GLib.Variant.new_string('clipboard'),
                ]);
                this._call(CAPTURE_IFACE, 'AddImage', params);
            });
        });
    }

    _hashBytes(data) {
        let h = 2166136261;
        for (let i = 0; i < data.length; i++)
            h = Math.imul(h ^ data[i], 16777619);
        return `${data.length}:${h >>> 0}`;
    }

    _fetchConfigBool(key, apply) {
        try {
            Gio.DBus.session.call(
                BUS_NAME, OBJECT_PATH, DAEMON_IFACE, 'GetConfig',
                new GLib.Variant('(s)', [key]),
                new GLib.VariantType('(s)'),
                Gio.DBusCallFlags.NONE, -1, null,
                (connection, res) => {
                    try {
                        const [value] = connection.call_finish(res).deepUnpack();
                        apply(value === 'true');
                    } catch (_e) {}
                }
            );
        } catch (_e) {}
    }

    _refreshConfig() {
        this._fetchConfigBool('ignore_images', (v) => {
            this._ignoreImages = v;
        });
        this._fetchConfigBool('popup_at_cursor', (v) => {
            this._popupAtCursor = v;
        });
    }

    _call(iface, method, params) {
        try {
            Gio.DBus.session.call(
                BUS_NAME, OBJECT_PATH, iface, method, params,
                null, Gio.DBusCallFlags.NONE, -1, null,
                (connection, res) => {
                    // Daemon not running yet → ignore; next copy will retry.
                    try {
                        connection.call_finish(res);
                    } catch (_e) {}
                }
            );
        } catch (_e) {
            // Never let a capture error propagate into the Shell.
        }
    }
}
