// Hidden admin panel (power level ≥ admin.min_pl): every tunable with default / current / reset,
// a simulator for tonight's odds under unsaved values, and the logged actions. DESIGN §4.4 / §6.
// 10.5.2 Credit section (the web's GameNightAdmin, same layout): the "Spins affect credits" switch
// (settings credits.mode, saved with Save), everyone's credit with − / + / Zero and Reset all credits
// — all written at "admin:<mxid>", so they land for everyone (they only ever reached the admin before).
import QtQuick
import dev.tinhat.app   // Th: the reel.* tokens
import QtQuick.Layouts
import QtQuick.Templates as T

Rectangle {
    id: adm
    required property var app
    required property var ctl
    radius: app.radiusLg
    color: Th.reelEditorBg
    border.color: Th.reelAdminBorder
    implicitHeight: col.implicitHeight + 28

    property var settings: ({})
    property var defaults: ({})
    property bool dirty: false
    function reload() {
        try { var s = JSON.parse(ctl.settingsJson || "{}"); settings = s.values || s; defaults = s.defaults || {} } catch (e) {}
        dirty = false
    }
    Component.onCompleted: reload()
    Connections { target: adm.ctl; function onSettingsJsonChanged() { if (!adm.dirty) adm.reload() } }
    function set(key, value) { var s = Object.assign({}, settings); s[key] = value; settings = s; dirty = true; simTimer.restart() }
    readonly property var sim: { try { return JSON.parse(ctl.simulationJson || "{}") } catch (e) { return {} } }
    // 10.5.2: everyone's banked credit (snapshot `credits`) + the credits.mode switch
    readonly property var people: { try { return JSON.parse(ctl.snapshotJson || "{}").credits || [] } catch (e) { return [] } }
    readonly property bool hasSpin: !!(ctl && ctl.snapshotJson.indexOf("\"spin\":{") >= 0)
    readonly property bool creditsEarn: (settings["credits.mode"] !== undefined ? settings["credits.mode"] : defaults["credits.mode"]) !== "free"
    Timer { id: simTimer; interval: 300; onTriggered: adm.ctl.simulate(JSON.stringify(adm.settings)) }

    readonly property var rows: [
        { k: "favorite.share", t: "Favorite share of a vote", min: 0.2, max: 0.9, step: 0.05 },
        { k: "regret.step", t: "Extra favorite weight per lost night", min: 0, max: 2, step: 0.1 },
        { k: "regret.cap", t: "Max credited nights (0 = off)", min: 0, max: 10, step: 1 },
        { k: "flexible.weight", t: "Weight a Flexible person spreads", min: 0, max: 0.5, step: 0.05 },
        { k: "veto.post_spin_per_person", t: "Post-spin vetoes per person", min: 0, max: 3, step: 1 },
        { k: "veto.respin_limit", t: "Re-spins per session", min: 0, max: 5, step: 1 },
        { k: "damping.factor", t: "Recent-winner weight (when damping is on)", min: 0.1, max: 1, step: 0.05 },
        { k: "reel.max_items", t: "Reel size (top N by weight)", min: 6, max: 30, step: 1 },
        { k: "reel.duration_ms", t: "Spin length (ms)", min: 2000, max: 12000, step: 500 },
        { k: "roster.grace_min", t: "Late-arrival grace (min)", min: 0, max: 30, step: 5 },
        { k: "admin.min_pl", t: "Power level to see this panel", min: 0, max: 100, step: 5 }
    ]
    // 10.5.1 (TW D8): Matrix (room v10) stores no fractions — a non-default fractional value can't
    // be saved (games_model::Settings::to_wire refuses it), so such a row is flagged before Save.
    readonly property string floatRefusal: "Matrix can't store fractional numbers yet — keep the default"
    function unstorable(k) {
        var v = Number(settings[k] !== undefined ? settings[k] : defaults[k])
        return !Number.isInteger(v) && v !== defaults[k]
    }
    readonly property var badRows: rows.filter(function (r) { return adm.unstorable(r.k) })
    readonly property var toggles: [
        { k: "feature.enabled", t: "Game night enabled (kill switch)" },
        { k: "pool.spin_stretch", t: "Spin the ‘most of us own’ tier too" },
        { k: "pool.count_flexible_ownership", t: "Flexible people count as owners" },
        { k: "regret.count_maybe", t: "A ‘maybe’ who showed up accrues credit" },
        { k: "regret.reset_on_favorite_change", t: "Changing favorite resets the streak" },
        { k: "damping.last_winner", t: "Damp recent winners" },
        { k: "steam.import", t: "Show the Steam import" },
        { k: "catalog.custom_entries", t: "Allow typed games not in the catalog" }
    ]

    ColumnLayout {
        id: col
        x: 14; y: 14
        width: parent.width - 28
        spacing: 10
        RowLayout {
            Layout.fillWidth: true
            Text { text: "Tune game night"; color: Th.reelFg; font.pixelSize: 16; font.weight: Font.DemiBold; Layout.fillWidth: true }
            Text { text: "rev " + (adm.settings.rev || 0) + (adm.dirty ? " · unsaved" : ""); color: adm.dirty ? Th.reelWarningFg : Th.reelMetaFg; font.pixelSize: 11 }
            ReelButton { text: "Reset all to defaults"; quiet: true; radius: app.radius; onClicked: { var s = Object.assign({}, adm.defaults); s.rev = adm.settings.rev; adm.settings = s; adm.dirty = true; simTimer.restart() } }
            ReelButton { text: "Save"; primary: true; enabled: adm.dirty && !ctl.busy && adm.badRows.length === 0; hint: adm.badRows.length > 0 ? adm.floatRefusal : ""; radius: app.radius; onClicked: { adm.ctl.saveSettings(JSON.stringify(adm.settings)); adm.dirty = false } }
        }
        Text { text: "Nothing here lets anyone choose a winner. Every change is logged with who and when (it is a state event)."; color: Th.reelMetaFg; font.pixelSize: 11; wrapMode: Text.WordWrap; Layout.fillWidth: true }
        Text { visible: adm.badRows.length > 0; text: adm.floatRefusal + ": " + adm.badRows.map(function (r) { return r.t }).join(", ") + "."; color: Th.reelWarningFg; font.pixelSize: 11; wrapMode: Text.WordWrap; Layout.fillWidth: true }
        GridLayout {
            Layout.fillWidth: true
            columns: 2
            columnSpacing: 16
            rowSpacing: 6
            Repeater {
                model: adm.rows
                delegate: RowLayout {
                    required property var modelData
                    Layout.fillWidth: true
                    spacing: 8
                    Text { text: modelData.t; color: Th.reelFg; font.pixelSize: 12; Layout.fillWidth: true; elide: Text.ElideRight }
                    T.SpinBox {
                        id: sb
                        readonly property real mult: modelData.step < 1 ? 100 : 1   // (not `scale`: Item's own — Qt 6.11 warns on the override)
                        from: Math.round(modelData.min * mult); to: Math.round(modelData.max * mult); stepSize: Math.round(modelData.step * mult)
                        value: Math.round(Number(adm.settings[modelData.k] !== undefined ? adm.settings[modelData.k] : (adm.defaults[modelData.k] || 0)) * mult)
                        onValueModified: adm.set(modelData.k, value / mult)
                        implicitWidth: 110; implicitHeight: 28
                        textFromValue: function (v) { return sb.mult === 1 ? String(v) : (v / sb.mult).toFixed(2) }
                        background: Rectangle { radius: app.radiusSm; color: Th.reelSpinboxBg; border.color: Th.reelSpinboxBorder }
                        contentItem: Text { text: sb.textFromValue(sb.value); color: Th.reelFg; font.pixelSize: 12; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
                        up.indicator: Rectangle { x: sb.width - width; width: 22; height: sb.height; color: "transparent"; Text { anchors.centerIn: parent; text: "+"; color: Th.reelSpinboxButtonFg } }
                        down.indicator: Rectangle { width: 22; height: sb.height; color: "transparent"; Text { anchors.centerIn: parent; text: "–"; color: Th.reelSpinboxButtonFg } }
                    }
                    Text { text: "(" + (adm.defaults[modelData.k] !== undefined ? adm.defaults[modelData.k] : "") + ")"; color: Th.reelMetaFg; font.pixelSize: 10; Layout.preferredWidth: 44 }
                }
            }
        }
        Flow {
            Layout.fillWidth: true
            spacing: 8
            Repeater {
                model: adm.toggles
                delegate: ReelButton {
                    required property var modelData
                    readonly property bool on: adm.settings[modelData.k] !== undefined ? !!adm.settings[modelData.k] : !!adm.defaults[modelData.k]
                    text: (on ? "☑ " : "☐ ") + modelData.t
                    checked: on
                    implicitHeight: 28
                    onClicked: adm.set(modelData.k, !on)
                    radius: app.radiusSm
                }
            }
        }
        // simulator
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 4
            Text { text: "Tonight under these values"; color: Th.reelSectionFg; font.pixelSize: 11; font.letterSpacing: 1; font.capitalization: Font.AllUppercase }
            Repeater {
                model: (adm.sim.tiers && adm.sim.tiers.main) ? adm.sim.tiers.main : []
                delegate: RowLayout {
                    required property var modelData
                    Layout.fillWidth: true
                    Text { text: modelData.name || modelData.id; color: Th.reelFg; font.pixelSize: 12; Layout.preferredWidth: 200; elide: Text.ElideRight }
                    Rectangle { Layout.fillWidth: true; height: 8; radius: 4; color: Th.reelOddsTrack; Rectangle { width: parent.width * Math.min(1, modelData.odds || 0); height: 8; radius: 4; color: Th.reelOddsFill } }
                    Text { text: Math.round((modelData.odds || 0) * 100) + "%"; color: Th.reelMetaFg; font.pixelSize: 11; Layout.preferredWidth: 40 }
                }
            }
            Text { visible: !(adm.sim.tiers && adm.sim.tiers.main && adm.sim.tiers.main.length); text: "Change a value to preview the odds."; color: Th.reelMetaFg; font.pixelSize: 11 }
        }
        // 10.5.2 credit
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 6
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Text { text: "Credit"; color: Th.reelSectionFg; font.pixelSize: 11; font.letterSpacing: 1; font.capitalization: Font.AllUppercase; Layout.fillWidth: true }
                ReelButton { text: "Reset all credits"; quiet: true; enabled: !adm.ctl.busy; onClicked: adm.ctl.resetCredits(); radius: app.radius; hint: "Writes a logged adjustment that zeroes everyone as of now" }
            }
            Flow {
                Layout.fillWidth: true
                spacing: 10
                ReelButton {
                    text: (adm.creditsEarn ? "☑ " : "☐ ") + "Spins affect credits"
                    checked: adm.creditsEarn
                    implicitHeight: 28
                    hint: "Saved with Save. Every spin records the mode it was spun in, so switching never changes past nights."
                    onClicked: adm.set("credits.mode", adm.creditsEarn ? "free" : "earn")
                    radius: app.radiusSm
                }
                Text {
                    height: 28
                    verticalAlignment: Text.AlignVCenter
                    text: adm.creditsEarn ? "A lost favourite earns credit; when it wins, the credit is spent." : "Free spins: nobody earns or spends credit, and credit doesn’t weight the reel."
                    color: Th.reelMetaFg; font.pixelSize: 11
                }
            }
            Text { visible: adm.people.length === 0; text: "Nobody has credit yet — it builds up as favourites lose."; color: Th.reelMetaFg; font.pixelSize: 11 }
            GridLayout {
                Layout.fillWidth: true
                columns: 2
                columnSpacing: 16
                rowSpacing: 6
                Repeater {
                    model: adm.people
                    delegate: RowLayout {
                        required property var modelData
                        Layout.fillWidth: true
                        Layout.preferredHeight: 28
                        spacing: 6
                        Text { text: modelData.name; color: Th.reelFg; font.pixelSize: 12; Layout.fillWidth: true; elide: Text.ElideRight }
                        Text { text: String(modelData.credit); color: modelData.hasFavorite ? Th.reelFg : Th.reelMetaFg; font.pixelSize: 13; font.weight: modelData.hasFavorite ? Font.DemiBold : Font.Normal; horizontalAlignment: Text.AlignRight; Layout.preferredWidth: 22 }
                        Text { text: modelData.offset ? (modelData.offset > 0 ? "+" : "") + modelData.offset + " adj." : ""; color: Th.reelMetaFg; font.pixelSize: 10; Layout.preferredWidth: 44 }
                        ReelButton { text: "–"; quiet: true; implicitWidth: 26; implicitHeight: 24; enabled: !adm.ctl.busy && modelData.credit > 0; hint: "−1 credit (logged)"; onClicked: adm.ctl.adjustCredit(modelData.mxid, -1, "Adjusted in Tune"); radius: app.radiusSm }
                        ReelButton { text: "+"; quiet: true; implicitWidth: 26; implicitHeight: 24; enabled: !adm.ctl.busy; hint: "+1 credit (logged) — spent when their favourite wins"; onClicked: adm.ctl.adjustCredit(modelData.mxid, 1, "Adjusted in Tune"); radius: app.radiusSm }
                        ReelButton { text: "Zero"; quiet: true; implicitHeight: 24; enabled: !adm.ctl.busy && !(modelData.credit === 0 && modelData.offset === 0); hint: "Back to 0 as of now (logged)"; onClicked: adm.ctl.zeroCredit(modelData.mxid); radius: app.radiusSm }
                    }
                }
            }
        }
        // actions
        RowLayout {
            Layout.fillWidth: true
            spacing: 8
            visible: adm.hasSpin
            ReelButton { visible: adm.hasSpin; text: "Void tonight’s spin"; quiet: true; radius: app.radius; onClicked: { try { var s = JSON.parse(adm.ctl.snapshotJson).spin; if (s && s.id) adm.ctl.voidSpin(s.id, "voided by admin") } catch (e) {} } }
        }
    }
}
