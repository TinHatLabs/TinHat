// EmojiPicker.qml — a small searchable emoji grid popover. Reused by the composer
// (insert into the message) and the message React action (send a reaction). Emits
// picked(emoji); the opener decides what to do with it. Themed via the app palette.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import dev.tinhat.app   // Th: the emoji.* tokens

Controls.Popup {
    id: picker
    property var app
    signal picked(string emoji)
    property bool stayOpen: false   // 10.1 (TH-CONV-15): the composer keeps it open for a run of picks; Escape / click-outside closes

    FallbackTheme { id: fallbackTheme }
    readonly property var theme: app || fallbackTheme

    modal: false   // 10.1 (TH-CONV-15): a picker should not dim the message you are reacting to
    dim: false
    focus: true
    closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
    width: 340
    height: 400
    padding: 0

    // Curated, keyword-tagged set covering the everyday categories (no asset pipeline).
    readonly property var allEmojis: [
        // smileys & emotion
        {e:"😀",k:"grin happy smile"},{e:"😁",k:"grin beam happy"},{e:"😂",k:"joy laugh tears lol"},
        {e:"🤣",k:"rofl rolling laugh"},{e:"😊",k:"blush smile happy"},{e:"😇",k:"innocent angel"},
        {e:"🙂",k:"slight smile"},{e:"😉",k:"wink"},{e:"😍",k:"heart eyes love"},{e:"🥰",k:"love hearts"},
        {e:"😘",k:"kiss love"},{e:"😜",k:"tongue wink silly"},{e:"🤪",k:"zany crazy goofy"},
        {e:"😎",k:"cool sunglasses"},{e:"🤩",k:"star struck wow"},{e:"🥳",k:"party celebrate"},
        {e:"😏",k:"smirk"},{e:"😒",k:"unamused meh"},{e:"😞",k:"sad disappointed"},{e:"😢",k:"cry sad tear"},
        {e:"😭",k:"sob cry bawl"},{e:"😤",k:"triumph steam huff"},{e:"😡",k:"angry mad rage"},
        {e:"🤬",k:"swear curse angry"},{e:"🤯",k:"mind blown shock"},{e:"😱",k:"scream shock fear"},
        {e:"😴",k:"sleep tired zzz"},{e:"🤔",k:"thinking hmm"},{e:"🤨",k:"raised eyebrow skeptic"},
        {e:"😐",k:"neutral meh"},{e:"😬",k:"grimace awkward"},{e:"🙄",k:"eye roll"},{e:"😳",k:"flushed embarrassed"},
        {e:"🥺",k:"pleading puppy eyes beg"},{e:"😅",k:"sweat smile nervous"},{e:"😆",k:"laugh squint"},
        {e:"🤗",k:"hug"},{e:"🤭",k:"giggle oops"},{e:"🤫",k:"shush quiet"},{e:"😋",k:"yum tasty"},
        {e:"🤤",k:"drool"},{e:"😷",k:"mask sick"},{e:"🤒",k:"sick thermometer"},{e:"🤮",k:"vomit sick"},
        {e:"🥵",k:"hot heat"},{e:"🥶",k:"cold freezing"},{e:"😵",k:"dizzy"},{e:"🤠",k:"cowboy"},
        // gestures & people
        {e:"👍",k:"thumbs up yes like good"},{e:"👎",k:"thumbs down no dislike bad"},{e:"👏",k:"clap applause"},
        {e:"🙌",k:"raised hands praise yay"},{e:"🙏",k:"pray thanks please"},{e:"🤝",k:"handshake deal"},
        {e:"💪",k:"muscle strong flex"},{e:"👌",k:"ok perfect"},{e:"🤌",k:"pinch chef italian"},
        {e:"✌️",k:"peace victory"},{e:"🤞",k:"fingers crossed luck"},{e:"🤙",k:"call shaka hang loose"},
        {e:"👋",k:"wave hi bye hello"},{e:"🫡",k:"salute respect"},{e:"🫶",k:"heart hands love"},
        {e:"👀",k:"eyes look watch"},{e:"🧠",k:"brain smart"},{e:"🫠",k:"melt"},
        // hearts & symbols
        {e:"❤️",k:"heart love red"},{e:"🧡",k:"orange heart"},{e:"💛",k:"yellow heart"},{e:"💚",k:"green heart"},
        {e:"💙",k:"blue heart"},{e:"💜",k:"purple heart"},{e:"🖤",k:"black heart"},{e:"🤍",k:"white heart"},
        {e:"💔",k:"broken heart"},{e:"💕",k:"hearts love"},{e:"💖",k:"sparkle heart"},{e:"💯",k:"hundred perfect"},
        {e:"✨",k:"sparkles shiny"},{e:"⭐",k:"star"},{e:"🌟",k:"glowing star"},{e:"🔥",k:"fire lit hot"},
        {e:"💥",k:"boom explosion"},{e:"⚡",k:"lightning fast power"},{e:"💫",k:"dizzy stars"},
        {e:"🎉",k:"party tada celebrate"},{e:"🎊",k:"confetti party"},{e:"🎈",k:"balloon"},{e:"🏆",k:"trophy win"},
        {e:"🥇",k:"gold medal first"},{e:"✅",k:"check yes done"},{e:"❌",k:"cross no wrong x"},
        {e:"❓",k:"question"},{e:"❗",k:"exclamation"},{e:"⚠️",k:"warning caution"},{e:"🚀",k:"rocket launch ship"},
        // animals & nature
        {e:"🐶",k:"dog puppy"},{e:"🐱",k:"cat"},{e:"🦊",k:"fox"},{e:"🐺",k:"wolf"},{e:"🐻",k:"bear"},
        {e:"🐼",k:"panda"},{e:"🐸",k:"frog"},{e:"🐵",k:"monkey"},{e:"🦁",k:"lion"},{e:"🐯",k:"tiger"},
        {e:"🐢",k:"turtle"},{e:"🐙",k:"octopus"},{e:"🦋",k:"butterfly"},{e:"🐝",k:"bee"},{e:"🦄",k:"unicorn"},
        {e:"🐉",k:"dragon"},{e:"🌹",k:"rose flower"},{e:"🌸",k:"blossom flower"},{e:"🌈",k:"rainbow"},
        {e:"🌙",k:"moon night"},{e:"☀️",k:"sun sunny"},{e:"⛄",k:"snowman winter"},{e:"🌊",k:"wave water ocean"},
        // food & drink
        {e:"🍕",k:"pizza"},{e:"🍔",k:"burger"},{e:"🍟",k:"fries"},{e:"🌮",k:"taco"},{e:"🍣",k:"sushi"},
        {e:"🍩",k:"donut"},{e:"🍪",k:"cookie"},{e:"🎂",k:"cake birthday"},{e:"🍰",k:"cake slice"},
        {e:"🍦",k:"icecream"},{e:"🍫",k:"chocolate"},{e:"🍿",k:"popcorn"},{e:"☕",k:"coffee tea"},
        {e:"🍺",k:"beer"},{e:"🍻",k:"cheers beers"},{e:"🍷",k:"wine"},{e:"🥂",k:"toast champagne"},
        {e:"🍾",k:"champagne pop"},{e:"🥤",k:"soda drink"},
        // activities & objects
        {e:"🎮",k:"game controller gaming"},{e:"🕹️",k:"joystick arcade"},{e:"🎧",k:"headphones music"},
        {e:"🎵",k:"music note"},{e:"🎤",k:"mic sing"},{e:"🎸",k:"guitar"},{e:"⚽",k:"soccer football"},
        {e:"🏀",k:"basketball"},{e:"🎯",k:"dart target bullseye"},{e:"🎲",k:"dice"},{e:"🏁",k:"finish flag race"},
        {e:"💻",k:"laptop computer"},{e:"🖥️",k:"desktop pc"},{e:"📱",k:"phone mobile"},{e:"💡",k:"idea bulb light"},
        {e:"🔑",k:"key"},{e:"🔒",k:"lock secure"},{e:"💰",k:"money bag"},{e:"💎",k:"diamond gem"},
        {e:"📷",k:"camera photo"},{e:"🎁",k:"gift present"},{e:"⏰",k:"alarm clock time"},{e:"💤",k:"sleep zzz afk"},
        {e:"🫥",k:"dotted invisible"},{e:"🤖",k:"robot bot"},{e:"👽",k:"alien"},{e:"💩",k:"poop"}
    ]

    property string query: ""
    readonly property var filtered: query.length === 0
        ? allEmojis
        : allEmojis.filter(function (x) { return x.k.indexOf(query.toLowerCase()) !== -1 || x.e === query })

    onAboutToShow: { query = ""; searchField.text = ""; searchField.forceActiveFocus() }

    background: Kirigami.ShadowedRectangle {
        radius: picker.theme.radius
        color: Th.emojiBg
        border.width: 1
        border.color: Th.emojiBorder
        shadow.size: 16
        shadow.color: Th.emojiShadow
        shadow.yOffset: 4
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: Kirigami.Units.smallSpacing
        spacing: Kirigami.Units.smallSpacing

        Kirigami.SearchField {
            // 9.2: themed from the app palette — a plain QQC2 style drew a white field over the
            // dark grid (Windows, the rig).
            Kirigami.Theme.inherit: false
            Kirigami.Theme.backgroundColor: Th.emojiSearchBg
            Kirigami.Theme.textColor: Th.emojiSearchFg
            palette.base: Th.emojiSearchBg
            palette.text: Th.emojiSearchFg
            palette.placeholderText: Th.emojiSearchPlaceholder
            palette.highlight: Th.emojiSearchBorderFocus
            color: Th.emojiSearchFg
            placeholderTextColor: Th.emojiSearchPlaceholder
            id: searchField
            Layout.fillWidth: true
            placeholderText: "Search emoji…"
            onTextChanged: picker.query = text
            Keys.onReturnPressed: {
                if (picker.filtered.length > 0) { picker.picked(picker.filtered[0].e); if (!picker.stayOpen) picker.close() }
            }
        }

        GridView {
            id: grid
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            cellWidth: 40
            cellHeight: 40
            model: picker.filtered
            Controls.Label {
                anchors.centerIn: parent
                visible: picker.filtered.length === 0
                text: "No emoji found — try another word"
                color: Th.emojiEmptyFg
            }
            delegate: Item {
                required property var modelData
                width: grid.cellWidth
                height: grid.cellHeight
                Rectangle {
                    anchors.fill: parent
                    anchors.margins: 2
                    radius: picker.theme.radiusSm
                    color: ema.containsMouse ? Th.emojiCellBgHover : "transparent"
                    Controls.Label {
                        textFormat: Text.PlainText   // security review 2026-10-26 (M7): never markup
                        anchors.centerIn: parent
                        text: modelData.e
                        font.pixelSize: 22
                    }
                    MouseArea {
                        id: ema
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: { picker.picked(modelData.e); if (!picker.stayOpen) picker.close() }
                    }
                }
            }
            Controls.ScrollBar.vertical: FoxScrollBar { app: picker.app }   // 10.1 (TH-DS-05): the house scrollbar everywhere
        }
    }
}
