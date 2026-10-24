pragma Singleton
// Seeds.qml — THE colour identity of an avatar (selfhost/firstrun §6; the web's src/ui/avatarSeed.ts, the same rule).
// Avatar's disc colour (and furry fox face) is a hash of its `seed`; every surface passes its seed through avatarSeed(),
// so one person — and one room — has one colour everywhere: chat, the room list, the DM strip, the room / DM header,
// call tiles, the quick switcher, floating chats, the room info panel. Before, a person was one colour in chat (their
// Matrix id) and another on their call tile (the LiveKit identity "@user:server:DEVICE"), and a room one colour in the
// list (its name) and another in its header (its id).
//   a person      their Matrix id; a LiveKit identity loses its device part
//   a DM          the other person (the DM looks like that person in the list, the strip and its header)
//   a room/space  its room id — never its name (a rename must not recolour it)
// SHARED LOGIC: tools/seeds-test.mjs runs the functions between the markers with the web's avatarSeed.test.ts cases.
import QtQuick

QtObject {
    id: seeds

    // BEGIN pure (tools/seeds-test.mjs)
    /// The Matrix user id inside a LiveKit participant identity: "@user:server:DEVICE" → "@user:server". A plain user
    /// id is returned as it is, a port included ("@user:server:8448"): the last part is a device only when it has a
    /// letter (Synapse device ids are letters; a port is digits) and no "]" (an IPv6 literal).
    function personSeed(id) {
        var s = String(id === undefined || id === null ? "" : id).trim()
        if (s.charAt(0) !== "@") return s
        var first = s.indexOf(":")
        var last = s.lastIndexOf(":")
        if (first < 0 || last === first) return s
        var tail = s.slice(last + 1)
        return (/[A-Za-z]/.test(tail) && tail.indexOf("]") < 0) ? s.slice(0, last) : s
    }
    /// The seed of an avatar: `dmPeer` (a DM's other person) wins; a person's id loses any device part; anything else
    /// (a room or space id) is used as it is.
    function avatarSeed(id, dmPeer) {
        var peer = String(dmPeer === undefined || dmPeer === null ? "" : dmPeer).trim()
        if (peer.length > 0) return seeds.personSeed(peer)
        return seeds.personSeed(id)
    }
    // END pure
}
