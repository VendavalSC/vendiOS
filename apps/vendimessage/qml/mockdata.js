// Mock conversation data for developing the UI before the real vendi-chatd
// backend is wired in. Every message has the SAME keys so the ListModel keeps
// stable roles: text, mine, time, kind, source, replyName, replyText, sender,
// senderColor. `sender`/`senderColor` are only shown in group chats.
.pragma library

function txt(sender, body, mine, time) {
    return { text: body, mine: mine, time: time, kind: "text", source: "",
             replyName: "", replyText: "", sender: sender, senderColor: "", reactions: "[]" };
}
function img(sender, src, mine, time) {
    return { text: "", mine: mine, time: time, kind: "image", source: src,
             replyName: "", replyText: "", sender: sender, senderColor: "", reactions: "[]" };
}
function reply(sender, body, mine, time, rName, rText) {
    return { text: body, mine: mine, time: time, kind: "text", source: "",
             replyName: rName, replyText: rText, sender: sender, senderColor: "", reactions: "[]" };
}
// a group message (carries the sender's colour so the name/avatar are tinted)
function gmsg(sender, color, body, mine, time) {
    return { text: body, mine: mine, time: time, kind: "text", source: "",
             replyName: "", replyText: "", sender: sender, senderColor: color, reactions: "[]" };
}

function conversations() {
    return [
        {
            id: "!armando", name: "Armando Cajide", color: "#5b7cfa",
            preview: "📷 Photo", time: "9:00 AM", unread: false,
            messages: [
                txt("Armando", "Hey!", false, "9:00 AM"),
                txt("Armando", "I got a new 🐶", false, "9:00 AM"),
                reply("me", "omg congrats!! 🐶", true, "9:00 AM", "Armando", "I got a new 🐶"),
                (function () { var m = txt("me", "It was great catching up with you the other day.", true, "9:01 AM"); m.reactions = "[\"❤️\"]"; return m; })(),
                txt("Armando", "look at this view from the trip 👇", false, "9:02 AM"),
                img("Armando", "../assets/sample1.jpg", false, "9:02 AM"),
                txt("me", "That's awesome! I can only imagine the fun you're having 😄", true, "9:03 AM"),
                txt("Armando", "🎉🥳🔥", false, "9:04 AM"),
                txt("me", "😂", true, "9:04 AM")
            ]
        },
        {
            id: "!trip", name: "Weekend Trip", color: "#34c759",
            preview: "Mia: i'm bringing the speaker 🔊", time: "9:12 AM", unread: true,
            group: true,
            members: [ { name: "Alex", color: "#5b7cfa" }, { name: "Mia", color: "#f0883e" },
                       { name: "Sam", color: "#bc6bd9" } ],
            messages: [
                gmsg("Alex", "#5b7cfa", "ok who's driving on saturday?", false, "9:05 AM"),
                gmsg("Mia", "#f0883e", "i can! got the big car", false, "9:06 AM"),
                txt("me", "perfect, i'll grab snacks 🍿", true, "9:07 AM"),
                gmsg("Sam", "#bc6bd9", "what time are we leaving", false, "9:10 AM"),
                gmsg("Alex", "#5b7cfa", "8am sharp 🙃", false, "9:11 AM"),
                gmsg("Mia", "#f0883e", "i'm bringing the speaker 🔊", false, "9:12 AM")
            ]
        },
        {
            id: "!ariel", name: "Ariel", color: "#f0883e",
            preview: "see you tomorrow!", time: "8:42 AM", unread: true,
            messages: [
                txt("Ariel", "are we still on for tomorrow?", false, "8:40 AM"),
                txt("me", "yep! 10am works", true, "8:41 AM"),
                txt("Ariel", "see you tomorrow!", false, "8:42 AM")
            ]
        },
        {
            id: "!zoe", name: "Zoe", color: "#bc6bd9",
            preview: "haha that's so true", time: "Yesterday", unread: false,
            messages: [
                txt("Zoe", "did you see the game last night??", false, "Yesterday"),
                img("Zoe", "../assets/sample2.jpg", false, "Yesterday"),
                txt("me", "unbelievable ending", true, "Yesterday"),
                txt("Zoe", "haha that's so true", false, "Yesterday")
            ]
        },
        {
            id: "!john", name: "John Appleseed", color: "#d9534f",
            preview: "See you at the park!", time: "Yesterday", unread: false,
            messages: [ txt("John", "See you at the park!", false, "Yesterday") ]
        }
    ];
}
