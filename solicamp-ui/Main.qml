import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15

// SoliCamp: write, compile, deploy and call Solidity on a local Solizone
// chain inside Basecamp, then optionally publish the chain to Bedrock.
// Every call goes to the solizone_module core module.
//
// Look: dark "cypherpunk" terminal palette, rounded modular cards, and the
// workflow labelled as numbered steps (01 write ... 05 publish).
Item {
    id: root
    width: 1280
    height: 860

    readonly property string mod: "solizone_module"

    // -- Theme --------------------------------------------------------------
    // Inline components below cannot see ids from this file, so they repeat
    // these values; change them in both places.
    QtObject {
        id: theme
        readonly property color ground: "#0A0C0B"
        readonly property color surface: "#0F1211"
        readonly property color raised: "#151B18"
        readonly property color terminal: "#070908"
        readonly property color line: "#1F2A24"
        readonly property color lineStrong: "#2C3A32"
        readonly property color text: "#D6E5DA"
        readonly property color body: "#A9BFB0"
        readonly property color muted: "#7F9A88"
        readonly property color gutter: "#3E5045"
        readonly property color accent: "#4BE38A"
        readonly property color accentInk: "#04140A"
        readonly property color cyan: "#5CD6E6"
        readonly property color amber: "#F2C14E"
        readonly property color red: "#FF6B5E"
        readonly property color violet: "#B99CFF"
    }

    readonly property string mono: Qt.fontFamilies().indexOf("JetBrains Mono") >= 0 ? "JetBrains Mono" : "monospace"

    // -- State --------------------------------------------------------------
    property bool running: false
    property string head: "-"
    property string chainId: ""
    property var contracts: []
    property int contractIndex: 0
    property string deployed: ""
    property var abiCache: ({})
    property string role: ""
    property string invite: ""
    property var net: ({ online: false })
    property string lastNote: ""
    property var pub: ({ state: "off" })
    property bool importOpen: false
    property var pendingTx: []
    property var rooms: []
    property var fns: []
    property bool leftOpen: true
    property string leftTab: "files"
    property bool rightOpen: false
    property string rightTab: "interact"
    property int editorSize: 15
    property var entries: []
    property var tree: []
    property var collapsed: ({})
    property string currentFile: ""
    property string selected: ""
    property bool dirty: false
    property bool loadingFile: false
    property bool seedPending: true
    property string fileAction: ""
    property string lastLine: ""
    property real outputRestore: 170

    readonly property string sample:
        "// SPDX-License-Identifier: MIT\n" +
        "pragma solidity ^0.8.20;\n\n" +
        "contract Counter {\n" +
        "    uint256 public count;\n\n" +
        "    function increment() public { count += 1; }\n" +
        "    function set(uint256 v) public { count = v; }\n" +
        "}\n"

    // -- Output log ---------------------------------------------------------
    ListModel { id: logModel }

    // Callers keep passing "✓ ...", "✗ ...", "⏳ ...", "→ ...", "ℹ ..."; the
    // leading glyph becomes a coloured tag.
    function log(text) {
        var tags = {
            "✓": ["[ok]", theme.accent],
            "✗": ["[err]", theme.red],
            "⏳": ["[..]", theme.amber],
            "→": ["[→]", theme.cyan],
            "ℹ": ["[i]", theme.muted]
        }
        var first = text.charAt(0)
        var tag = tags[first]
        var rest = tag ? text.substring(1).trim() : text
        logModel.append({ tag: tag ? tag[0] : "", tone: tag ? tag[1].toString() : theme.muted.toString(), line: rest })
        lastLine = (tag ? tag[0] + " " : "") + rest
        outputList.positionViewAtEnd()
    }

    // The bridge hands back the module's JSON text. Depending on the host it may
    // be wrapped in one more JSON string, so unwrap until it is an object.
    // The output panel opens and closes from the arrow in its header (or by
    // dragging its edge). It remembers how tall it was.
    function toggleOutput() {
        if (outputCard.height > 70) {
            outputRestore = outputCard.height
            outputCard.SplitView.preferredHeight = 42
        } else {
            outputCard.SplitView.preferredHeight = Math.max(outputRestore, 120)
        }
    }

    function parse(raw) {
        var value = raw
        for (var i = 0; i < 3 && typeof value === "string"; i++) {
            try { value = JSON.parse(value) } catch (e) { return { error: "unreadable reply: " + value } }
        }
        return value || { error: "empty reply" }
    }

    function call(method, args, done, failed) {
        logos.callModuleAsync(mod, method, args, function (raw) {
            var reply = parse(raw)
            if (reply.error) {
                log("✗ " + method + ": " + (reply.error.message || reply.error))
                if (failed) failed(reply)
                return
            }
            done(reply)
        })
    }

    function loadAccounts() {
        call("accounts", [], function (a) {
            accountBox.model = a.accounts.map(function (x) {
                return "#" + x.index + " · " + shorten(x.address)
            })
        })
    }

    function refresh() {
        call("status", [], function (s) {
            var was = running
            running = s.running === true
            if (running && (!was || accountBox.count === 0)) loadAccounts()
            if (running && !was) loadFiles("")
            if (running && rooms.length === 0) loadRooms()
            head = running ? "#" + s.head : "-"
            chainId = running && s.chainId !== undefined ? String(s.chainId) : ""
            role = (running && s.collab) ? s.collab.role : ""
        })
    }

    function openRoomId() {
        for (var i = 0; i < rooms.length; i++)
            if (rooms[i].open) return rooms[i].id
        return ""
    }

    function openRoomName() {
        for (var i = 0; i < rooms.length; i++)
            if (rooms[i].open) return rooms[i].name
        return ""
    }

    function resetRoomState() {
        deployed = ""
        contracts = []
        contractBox.model = []
        fns = []
        invite = ""
        pendingTx = []
        pub = ({ state: "off" })
        importOpen = false
        pubPoll()
        saveTimer.stop()
        currentFile = ""
        selected = ""
        dirty = false
        fileAction = ""
        entries = []
        tree = []
        collapsed = ({})
        seedPending = true
        setEditorText("")
        loadFiles("")
    }

    // Share the open room: show its invite and go online so people can join.
    function shareRoom() {
        var id = openRoomId()
        if (id === "" || id === "main") { log("✗ open a room first (Main cannot be shared)"); return }
        call("share_room", [id], function (r) {
            invite = r.invite
            role = r.role
            log("✓ sharing this room as " + r.role + "; send the invite to the people you work with")
            goOnline()
        })
    }

    function joinRoom() {
        var text = inviteInput.text.trim()
        if (text === "") { log("✗ paste an invite first"); return }
        call("join_room", [text], function (s) {
            log("✓ joined room " + s.room.name + " (chain " + s.chainId + "); waiting for the host's chain")
            inviteInput.text = ""
            resetRoomState()
            refresh()
            loadRooms()
            goOnline()
        })
    }

    function goOnline() {
        call("net_start", [], function (n) { net = n })
    }

    function goOffline() {
        call("net_stop", [], function (n) {
            net = { online: false }
            log("✓ offline; your rooms and chains are untouched")
        })
    }

    function netPoll() {
        call("net_status", [], function (n) {
            net = n
            if (n.note && n.note !== lastNote) {
                lastNote = n.note
                log("ℹ " + n.note)
            }
        })
    }

    // -- Publish to Bedrock ----------------------------------------------
    // The local chain is a sandbox. Nothing reaches the Logos network, and no
    // fee is paid, until the user presses "Publish to Bedrock".

    function pubPoll() {
        call("publish_status", [], function (p) { if (!p.error) pub = p })
    }

    function pubStart() {
        call("publish_start", [], function (p) {
            if (p.error) { log("✗ " + p.error); return }
            pub = p
            log("✓ publishing is on for this room: " + p.message)
        })
    }

    // Choose which wallet address pays the fees; empty goes back to automatic.
    function pubUseAddress(address) {
        call("publish_use_address", [address], function (p) {
            if (p.error) { log("✗ " + p.error); return }
            pub = p
            log(address === "" ? "✓ the fee address is chosen automatically again"
                               : "✓ fees are paid from " + shorten(address))
        })
    }

    function balanceText() {
        if (pub.balance !== undefined && pub.balance !== null) return String(pub.balance)
        return "unreadable"
    }

    function pubStop() {
        call("publish_stop", [], function (p) {
            if (p.error) { log("✗ " + p.error); return }
            pub = p
            log("✓ publishing is off; the local chain is untouched")
        })
    }

    function pubImport() {
        call("publish_import", [chanInput.text, keyInput.text], function (r) {
            if (r.error) { log("✗ " + r.error); return }
            chanInput.text = ""
            keyInput.text = ""
            importOpen = false
            log("✓ this room now publishes to channel " + r.channelId)
            pubPoll()
        })
    }

    function pubTag() {
        var tags = {
            off: "off", guest: "guest", needs_node: "needs node", needs_funding: "needs funds",
            syncing: "syncing", published: "published", error: "stopped"
        }
        return tags[pub.state] || pub.state
    }

    function pubTone() {
        switch (pub.state) {
        case "needs_node":
        case "needs_funding": return theme.amber
        case "syncing": return theme.cyan
        case "published": return theme.accent
        case "error": return theme.red
        default: return theme.muted
        }
    }

    function pubHasProgress() {
        return (pub.state === "syncing" || pub.state === "published")
            && pub.head !== undefined && pub.head !== null
    }

    function pubBlock() {
        return pub.nextHeight > 0 ? pub.nextHeight - 1 : 0
    }

    function pubPercent() {
        if (!pubHasProgress() || !(pub.head > 0)) return 0
        return Math.min(100, Math.round(pubBlock() / pub.head * 100))
    }

    function pubFooter() {
        switch (pub.state) {
        case "off": return "local sandbox · nothing published"
        case "guest": return "guest · the host publishes"
        case "needs_node": return "publishing · waiting for the logos node"
        case "needs_funding": return "publishing · needs funding"
        case "error": return "publishing stopped"
        default:
            return (pub.state === "published" ? "published to logos" : "publishing")
                + (pubHasProgress() ? " · block " + pubBlock() + " / " + pub.head : "")
        }
    }

    function netText() {
        if (!net.online) return "p2p offline"
        var text = "p2p " + (net.phase || "online") + (net.connection ? " · " + net.connection : "")
        if (net.topic) text += " · sent " + net.sent + " · recv " + net.received
        if (net.error) text += " · ! " + net.error
        return text
    }

    // A guest's transactions are ordered by the host; wait for its block.
    function awaitReceipt(hash, done) {
        var list = pendingTx.slice()
        list.push({ hash: hash, done: done })
        pendingTx = list
    }

    function checkPending() {
        var list = pendingTx.slice()
        for (var i = 0; i < list.length; i++) {
            (function (item) {
                call("receipt", [item.hash], function (r) {
                    if (r.pending) return
                    pendingTx = pendingTx.filter(function (p) { return p.hash !== item.hash })
                    item.done(r)
                })
            })(list[i])
        }
    }

    function loadRooms() {
        call("list_rooms", [], function (r) { rooms = r.rooms })
    }

    function switchRoom(index) {
        if (index < 0 || index >= rooms.length || rooms[index].open) return
        call("open_room", [rooms[index].id], function (s) {
            log("✓ opened room " + s.room.name + " (chain " + s.chainId + ", head #" + s.head + ")")
            resetRoomState()
            refresh()
            loadRooms()
        })
    }

    function newRoom() {
        var name = roomName.text.trim()
        if (name === "") { log("✗ give the room a name"); return }
        call("create_room", [name], function (r) {
            log("✓ created room " + r.name + " (chain " + r.chainId + ")")
            roomName.text = ""
            loadRooms()
        })
    }

    function startChain() {
        // An empty directory means the module's default (~/.solicamp).
        call("start", [""], function (s) {
            running = true
            log("✓ chain " + s.chainId + (s.fresh ? " created" : " resumed") + ", head #" + s.head)
            loadAccounts()
            refresh()
            loadRooms()
        })
    }

    // Saves the open file first, so the compiler sees what is on screen, and
    // compiles it together with the files it imports.
    function compile() {
        flush(function () {
            if (currentFile === "") call("compile", [editor.text], showCompiled)
            else call("compile_file", [currentFile], showCompiled)
        })
    }

    function showCompiled(r) {
        var all = r.contracts
        var mine = all.filter(function (c) { return c.file === currentFile })
        var rest = all.filter(function (c) { return c.file !== currentFile })
        var seen = {}
        contracts = mine.concat(rest)
        contracts.forEach(function (c) { seen[c.file || ""] = true })
        var several = Object.keys(seen).length > 1
        contractBox.model = contracts.map(function (c) {
            return c.name + (several && c.file ? "  ·  " + baseName(c.file) : "")
        })
        contractIndex = 0
        loadFunctions()
        log("✓ compiled " + contracts.length + " contract(s) with " + r.solc)
    }

    function contractName() {
        var c = contracts[contractBox.currentIndex]
        return c ? c.name : ""
    }

    function currentAbi() {
        var abi = contracts[contractBox.currentIndex].abi
        return typeof abi === "string" ? abi : JSON.stringify(abi)
    }

    // One button per ABI function, marked read (view/pure) or write.
    function loadFunctions() {
        if (contracts.length === 0) { fns = []; return }
        var abi = contracts[contractBox.currentIndex].abi
        if (typeof abi === "string") {
            try { abi = JSON.parse(abi) } catch (e) { fns = []; return }
        }
        fns = abi.filter(function (f) { return f.type === "function" }).map(function (f) {
            var inputs = f.inputs || []
            var read = f.stateMutability === "view" || f.stateMutability === "pure" || f.constant === true
            return {
                name: f.name,
                sig: f.name + "(" + inputs.map(function (i) { return i.type }).join(",") + ")",
                kind: read ? "read" : "write",
                hint: inputs.length === 0 ? "no arguments"
                    : inputs.map(function (i) { return (i.name || "arg") + " (" + i.type + ")" }).join(", ")
            }
        })
        if (fns.length > 0 && fnInfo(fnName.text) === null) fnName.text = fns[0].name
    }

    function fnInfo(name) {
        for (var i = 0; i < fns.length; i++)
            if (fns[i].name === name.trim()) return fns[i]
        return null
    }

    function shorten(hex) {
        if (!hex || hex.length < 14) return hex || ""
        return hex.substring(0, 6) + "…" + hex.substring(hex.length - 4)
    }

    function lineNumbers(count) {
        var text = ""
        for (var i = 1; i <= Math.max(count, 1); i++) text += (i > 1 ? "\n" : "") + i
        return text
    }

    // -- Contract files -----------------------------------------------------
    // Files are plain .sol files in the room's contracts folder, kept by the
    // core module. The open file saves itself shortly after you stop typing.

    function parentOf(path) {
        var i = path.lastIndexOf("/")
        return i < 0 ? "" : path.substring(0, i)
    }

    function baseName(path) {
        return path.substring(path.lastIndexOf("/") + 1)
    }

    function hasEntry(path) {
        for (var i = 0; i < entries.length; i++)
            if (entries[i].path === path) return true
        return false
    }

    function isDir(path) {
        for (var i = 0; i < entries.length; i++)
            if (entries[i].path === path) return entries[i].kind === "dir"
        return false
    }

    function firstFile() {
        for (var i = 0; i < entries.length; i++)
            if (entries[i].kind === "file") return entries[i].path
        return ""
    }

    // The folder new things go in: the selected folder, or the folder of the
    // selected file.
    function targetDir() {
        if (selected === "") return ""
        return isDir(selected) ? selected : parentOf(selected)
    }

    function inDir(dir, name) {
        return dir === "" ? name : dir + "/" + name
    }

    function setEditorText(text) {
        loadingFile = true
        editor.text = text
        loadingFile = false
    }

    function template(file) {
        var name = file.replace(/\.sol$/, "").replace(/[^A-Za-z0-9_]/g, "_")
        if (name === "" || /^[0-9]/.test(name)) name = "C" + name
        return "// SPDX-License-Identifier: MIT\npragma solidity ^0.8.20;\n\ncontract " + name + " {\n}\n"
    }

    function buildTree() {
        var out = []
        for (var i = 0; i < entries.length; i++) {
            var e = entries[i]
            var parts = e.path.split("/")
            var hidden = false
            for (var k = 1; k < parts.length; k++) {
                if (collapsed[parts.slice(0, k).join("/")] === true) { hidden = true; break }
            }
            if (hidden) continue
            out.push({
                path: e.path,
                name: parts[parts.length - 1],
                depth: parts.length - 1,
                dir: e.kind === "dir",
                closed: e.kind === "dir" && collapsed[e.path] === true
            })
        }
        tree = out
    }

    function setCollapsed(path, value) {
        var next = {}
        for (var key in collapsed) next[key] = collapsed[key]
        next[path] = value
        collapsed = next
    }

    // Opens every folder above `path` so it can be seen.
    function reveal(path) {
        var parts = path.split("/")
        for (var k = 1; k < parts.length; k++) setCollapsed(parts.slice(0, k).join("/"), false)
        buildTree()
    }

    function loadFiles(openPath) {
        call("fs_list", [], function (r) {
            entries = r.entries
            if (entries.length === 0 && seedPending && openPath === "") {
                seedPending = false
                call("fs_create", ["Counter.sol", sample], function (c) { loadFiles(c.path) })
                return
            }
            seedPending = false
            buildTree()
            if (openPath !== "") { openFile(openPath); return }
            if (currentFile !== "" && hasEntry(currentFile)) return
            var first = firstFile()
            if (first !== "") {
                openFile(first)
            } else {
                currentFile = ""
                dirty = false
                setEditorText("")
            }
        })
    }

    function openFile(path) {
        flush(function () {
            call("fs_read", [path], function (r) {
                setEditorText(r.content)
                currentFile = r.path
                selected = r.path
                dirty = false
                contracts = []
                contractBox.model = []
                fns = []
                reveal(r.path)
            })
        })
    }

    function edited() {
        if (loadingFile || currentFile === "") return
        dirty = true
        saveTimer.restart()
    }

    // Writes the open file if it has unsaved changes, then calls `done`.
    function flush(done) {
        saveTimer.stop()
        if (!dirty || currentFile === "") {
            if (done) done()
            return
        }
        var path = currentFile
        dirty = false
        call("fs_write", [path, editor.text], function () {
            if (done) done()
        }, function () {
            dirty = true
        })
    }

    function clickEntry(entry) {
        selected = entry.path
        fileAction = ""
        if (entry.dir) {
            setCollapsed(entry.path, !entry.closed)
            buildTree()
        } else if (entry.path !== currentFile) {
            openFile(entry.path)
        }
    }

    function startFileAction(kind) {
        if (!running) { log("✗ start the chain first"); return }
        if ((kind === "rename" || kind === "delete") && selected === "") return
        fileAction = kind
        actionInput.text = kind === "rename" ? baseName(selected) : ""
        if (kind !== "delete") actionInput.forceActiveFocus()
    }

    function runFileAction() {
        var kind = fileAction
        if (kind === "delete") { deleteSelected(); return }

        var name = actionInput.text.trim()
        if (name === "") return
        var dir = targetDir()

        if (kind === "newfile") {
            if (!/\.sol$/.test(name)) name += ".sol"
            call("fs_create", [inDir(dir, name), template(baseName(name))], function (r) {
                fileAction = ""
                loadFiles(r.path)
            })
        } else if (kind === "newfolder") {
            call("fs_mkdir", [inDir(dir, name)], function (r) {
                fileAction = ""
                selected = r.path
                reveal(r.path + "/x")
                loadFiles("")
            })
        } else if (kind === "rename") {
            if (!isDir(selected) && !/\.sol$/.test(name)) name += ".sol"
            renameEntry(selected, inDir(parentOf(selected), name), function () { fileAction = "" })
        }
    }

    function renameEntry(from, to, done) {
        flush(function () {
            call("fs_rename", [from, to], function (r) {
                if (currentFile === from) currentFile = r.to
                else if (currentFile.indexOf(from + "/") === 0) currentFile = r.to + currentFile.substring(from.length)
                selected = r.to
                log("✓ moved " + r.from + " → " + r.to)
                if (done) done()
                reveal(r.to)
                loadFiles("")
            })
        })
    }

    // Drag a row onto a folder (or onto a file, to use its folder, or onto the
    // empty area for the top level).
    function moveEntry(source, targetDirPath) {
        if (source === "") return
        var dest = inDir(targetDirPath, baseName(source))
        if (dest === source) return
        renameEntry(source, dest, null)
    }

    function deleteSelected() {
        var path = selected
        call("fs_delete", [path], function () {
            if (currentFile === path || currentFile.indexOf(path + "/") === 0) {
                saveTimer.stop()
                currentFile = ""
                dirty = false
            }
            selected = ""
            fileAction = ""
            log("✓ deleted " + path)
            loadFiles("")
        })
    }

    function freeName(dir, name) {
        var dot = name.lastIndexOf(".")
        var stem = dot > 0 ? name.substring(0, dot) : name
        for (var n = 0; n < 100; n++) {
            var candidate = inDir(dir, stem + (n === 0 ? "" : "-" + n) + ".sol")
            if (!hasEntry(candidate)) return candidate
        }
        return inDir(dir, stem + "-" + Date.now() + ".sol")
    }

    // Dropping text always works; dropping a file from Finder needs file
    // access that Basecamp may not grant a view.
    function handleDrop(drop) {
        var urls = drop.urls || []
        if (urls.length > 0 && String(urls[0]).indexOf("file:") === 0) {
            loadFile(String(urls[0]))
            return
        }
        if (drop.hasText && drop.text) loaded(drop.text, "Dropped.sol")
    }

    // A dropped file or text becomes a new file next to the one you are in.
    function loaded(text, name) {
        if (!running) {
            setEditorText(text)
            log("✓ loaded " + name + "; start the chain to save it as a file")
            return
        }
        var path = freeName(targetDir(), name)
        call("fs_create", [path, text], function (r) {
            log("✓ saved the dropped code as " + r.path)
            loadFiles(r.path)
        })
    }

    function loadFile(url) {
        var name = decodeURIComponent(url.substring(url.lastIndexOf("/") + 1))
        var failed = "✗ could not read " + name + " here; open it, select the code and drag the text in, or paste it"
        if (!/\.sol$/.test(name)) { log("✗ only .sol files can be dropped"); return }
        try {
            var xhr = new XMLHttpRequest()
            xhr.open("GET", url)
            xhr.onreadystatechange = function () {
                if (xhr.readyState !== 4) return
                if (xhr.responseText && xhr.responseText.length > 0) loaded(xhr.responseText, name)
                else log(failed)
            }
            xhr.send()
        } catch (e) {
            log(failed)
        }
    }

    function deploy() {
        var c = contracts[contractBox.currentIndex]
        call("deploy", [accountBox.currentIndex, c.bytecode, ""], function (r) {
            if (r.pending) {
                log("⏳ deploy sent to the host; waiting for its block (" + r.txHash + ")")
                awaitReceipt(r.txHash, function (done) { finishDeploy(c, done) })
                return
            }
            finishDeploy(c, r)
        })
    }

    // Side panels open and close around the editor. Clicking the active
    // button again closes the panel.
    function toggleLeft(tab) {
        if (leftOpen && leftTab === tab) {
            leftOpen = false
        } else {
            leftTab = tab
            leftOpen = true
        }
    }

    function toggleRight(tab) {
        if (rightOpen && rightTab === tab) {
            rightOpen = false
        } else {
            rightTab = tab
            rightOpen = true
        }
    }

    function finishDeploy(c, r) {
        if (!r.success) { log("✗ deploy reverted in " + r.txHash); return }
        deployed = r.contractAddress
        log("✓ " + c.name + " deployed at " + deployed + " (block " + parseInt(r.blockNumber) + ")")
        rightTab = "interact"
        rightOpen = true
        refresh()
    }

    function finishSend(name, r) {
        log((r.success ? "✓ " : "✗ ") + name + " in block " + parseInt(r.blockNumber) + " (" + r.txHash + ")")
        refresh()
    }

    function invoke(write) {
        if (deployed === "") { log("✗ deploy a contract first"); return }
        var name = fnName.text.trim()
        var args = "[" + fnArgs.text + "]"

        call("encode", [currentAbi(), name, args], function (e) {
            if (write) {
                call("send", [accountBox.currentIndex, deployed, e.data, "0"], function (r) {
                    if (r.pending) {
                        log("⏳ " + name + " sent to the host; waiting for its block (" + r.txHash + ")")
                        awaitReceipt(r.txHash, function (done) { finishSend(name, done) })
                        return
                    }
                    finishSend(name, r)
                })
            } else {
                call("read", [deployed, e.data], function (r) {
                    call("decode", [currentAbi(), name, r.result], function (d) {
                        log("→ " + name + " = " + JSON.stringify(d.values))
                    })
                })
            }
        })
    }

    Component.onCompleted: {
        editor.text = sample
        refresh()
    }

    Timer {
        interval: 2000
        repeat: true
        running: root.running
        onTriggered: { root.refresh(); if (root.net.online) root.netPoll() }
    }

    Timer {
        interval: 5000
        repeat: true
        running: root.running
        onTriggered: root.pubPoll()
    }

    Timer {
        id: saveTimer
        interval: 700
        repeat: false
        onTriggered: root.flush(null)
    }

    Shortcut {
        sequence: "Ctrl+S"
        onActivated: root.flush(function () { root.log("✓ saved") })
    }

    Timer {
        interval: 1000
        repeat: true
        running: root.pendingTx.length > 0
        onTriggered: root.checkPending()
    }

    // -- Building blocks ----------------------------------------------------

    // kind: "primary" (filled accent), "accent" (accent outline),
    // "outline" (neutral) or "ghost" (no frame).
    component Btn: Button {
        id: btn
        property string kind: "outline"
        readonly property string monoFont: Qt.fontFamilies().indexOf("JetBrains Mono") >= 0 ? "JetBrains Mono" : "monospace"
        implicitHeight: 38
        leftPadding: 14
        rightPadding: 14
        font.family: monoFont
        font.pixelSize: 13
        font.bold: kind === "primary" || kind === "accent"
        hoverEnabled: true
        contentItem: Text {
            text: btn.text
            font: btn.font
            color: btn.kind === "primary" ? "#04140A" : (btn.kind === "accent" ? "#4BE38A" : "#D6E5DA")
            opacity: btn.enabled ? 1 : 0.45
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }
        background: Rectangle {
            radius: 8
            opacity: btn.enabled ? 1 : 0.45
            color: btn.kind === "primary" ? (btn.down ? "#3BC274" : "#4BE38A")
                 : btn.kind === "ghost" ? (btn.hovered ? "#151B18" : "transparent")
                 : (btn.down ? "#1C2420" : btn.hovered ? "#151B18" : "transparent")
            border.width: btn.kind === "ghost" ? 0 : 1
            border.color: btn.kind === "primary" || btn.kind === "accent" ? "#4BE38A"
                        : (btn.visualFocus ? "#5CD6E6" : "#2C3A32")
        }
    }

    component Field: TextField {
        id: field
        readonly property string monoFont: Qt.fontFamilies().indexOf("JetBrains Mono") >= 0 ? "JetBrains Mono" : "monospace"
        implicitHeight: 38
        leftPadding: 10
        rightPadding: 10
        font.family: monoFont
        font.pixelSize: 13
        color: "#D6E5DA"
        placeholderTextColor: "#7F9A88"
        selectionColor: "#2E5C44"
        selectedTextColor: "#D6E5DA"
        selectByMouse: true
        // Read-only values (invite, wallet) show their start, not their end.
        onTextChanged: if (readOnly) cursorPosition = 0
        background: Rectangle {
            radius: 3
            color: field.readOnly ? "#0F1211" : "#0A0C0B"
            border.color: field.activeFocus ? "#5CD6E6" : "#2C3A32"
            opacity: field.enabled ? 1 : 0.5
        }
    }

    component Pick: ComboBox {
        id: pick
        readonly property string monoFont: Qt.fontFamilies().indexOf("JetBrains Mono") >= 0 ? "JetBrains Mono" : "monospace"
        implicitHeight: 38
        font.family: monoFont
        font.pixelSize: 13
        contentItem: Text {
            leftPadding: 10
            rightPadding: 26
            text: pick.displayText
            font: pick.font
            color: "#D6E5DA"
            opacity: pick.enabled ? 1 : 0.5
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }
        indicator: Text {
            x: pick.width - width - 10
            y: (pick.height - height) / 2
            text: "▾"
            color: "#7F9A88"
            font.pixelSize: 11
        }
        background: Rectangle {
            radius: 3
            color: "#0A0C0B"
            border.color: pick.activeFocus ? "#5CD6E6" : "#2C3A32"
            opacity: pick.enabled ? 1 : 0.5
        }
        delegate: ItemDelegate {
            width: pick.width
            highlighted: pick.highlightedIndex === index
            contentItem: Text {
                text: modelData
                font: pick.font
                color: "#D6E5DA"
                elide: Text.ElideRight
                verticalAlignment: Text.AlignVCenter
            }
            background: Rectangle { radius: 3; color: highlighted ? "#151B18" : "transparent" }
        }
        popup: Popup {
            y: pick.height + 4
            width: pick.width
            padding: 4
            implicitHeight: Math.min(contentItem.implicitHeight + 8, 260)
            contentItem: ListView {
                clip: true
                implicitHeight: contentHeight
                model: pick.popup.visible ? pick.delegateModel : null
                currentIndex: pick.highlightedIndex
            }
            background: Rectangle { radius: 3; color: "#0F1211"; border.color: "#2C3A32" }
        }
    }

    component Zoom: Slider {
        id: zoom
        implicitHeight: 26
        background: Rectangle {
            x: zoom.leftPadding
            y: zoom.topPadding + zoom.availableHeight / 2 - height / 2
            width: zoom.availableWidth
            height: 4
            radius: 2
            color: "#2C3A32"
            Rectangle { width: zoom.visualPosition * parent.width; height: parent.height; radius: 2; color: "#4BE38A" }
        }
        handle: Rectangle {
            x: zoom.leftPadding + zoom.visualPosition * (zoom.availableWidth - width)
            y: zoom.topPadding + zoom.availableHeight / 2 - height / 2
            width: 10
            height: 18
            radius: 2
            color: zoom.pressed ? "#3BC274" : "#4BE38A"
        }
    }

    component TabBtn: Button {
        id: tb
        property bool active: false
        property bool dot: false
        property color dotColor: "#7F9A88"
        readonly property string monoFont: Qt.fontFamilies().indexOf("JetBrains Mono") >= 0 ? "JetBrains Mono" : "monospace"
        implicitHeight: 34
        leftPadding: 14
        rightPadding: 14
        hoverEnabled: true
        font.family: monoFont
        font.pixelSize: 13
        font.bold: active
        contentItem: Row {
            spacing: 8
            Text { text: tb.text; font: tb.font; color: tb.active ? "#D6E5DA" : "#7F9A88"; anchors.verticalCenter: parent.verticalCenter }
            Rectangle { visible: tb.dot; width: 8; height: 8; radius: 4; color: tb.dotColor; anchors.verticalCenter: parent.verticalCenter }
        }
        background: Rectangle {
            radius: 8
            color: tb.active ? "#151B18" : (tb.hovered ? "#121815" : "transparent")
            border.color: tb.active ? "#2C3A32" : "transparent"
        }
    }

    component DrawerTabs: RowLayout {
        id: dt
        property var names: []
        property string current: ""
        property string arrow: "‹"
        signal picked(string name)
        signal collapse()
        spacing: 6
        Repeater {
            model: dt.names
            delegate: TabBtn { text: modelData; active: dt.current === modelData; onClicked: dt.picked(modelData) }
        }
        Item { Layout.fillWidth: true }
        Btn { text: dt.arrow; kind: "ghost"; implicitHeight: 30; leftPadding: 10; rightPadding: 10; onClicked: dt.collapse() }
    }

    component Card: Rectangle {
        radius: 4
        color: "#0F1211"
        border.color: "#1F2A24"
        border.width: 1
    }

    // "> rooms", "04 interact": the section labels.
    component Heading: Label {
        readonly property string monoFont: Qt.fontFamilies().indexOf("JetBrains Mono") >= 0 ? "JetBrains Mono" : "monospace"
        font.family: monoFont
        font.pixelSize: 13
        font.bold: true
        color: "#4BE38A"
    }

    component Hint: Label {
        color: "#7F9A88"
        font.pixelSize: 13
        wrapMode: Text.WordWrap
    }

    component Pill: Rectangle {
        id: pill
        property alias text: pillText.text
        property color tone: "#7F9A88"
        property bool dot: true
        readonly property string monoFont: Qt.fontFamilies().indexOf("JetBrains Mono") >= 0 ? "JetBrains Mono" : "monospace"
        implicitHeight: 22
        implicitWidth: pillRow.implicitWidth + 20
        radius: 3
        color: "transparent"
        border.color: tone
        Row {
            id: pillRow
            anchors.centerIn: parent
            spacing: 6
            Rectangle {
                visible: pill.dot
                width: 6; height: 6; radius: 3
                color: pill.tone
                anchors.verticalCenter: parent.verticalCenter
            }
            Text {
                id: pillText
                color: pill.tone
                font.family: pill.monoFont
                font.pixelSize: 11
                anchors.verticalCenter: parent.verticalCenter
            }
        }
    }

    // -- Screen -------------------------------------------------------------

    Rectangle { anchors.fill: parent; color: theme.ground }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Top bar: identity, rooms toggle, chain state, signer.
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 56
            color: theme.surface
            Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: theme.line }

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 16
                anchors.rightMargin: 16
                spacing: 12

                Rectangle {
                    implicitWidth: scMark.implicitWidth + 12
                    implicitHeight: 24
                    radius: 3
                    color: "transparent"
                    border.color: theme.accent
                    Text { id: scMark; anchors.centerIn: parent; text: "[sc]"; color: theme.accent; font.family: root.mono; font.pixelSize: 13; font.bold: true }
                }
                Label { text: "solicamp"; color: theme.text; font.family: root.mono; font.pixelSize: 15; font.bold: true }

                Btn {
                    kind: root.leftOpen && root.leftTab === "files" ? "accent" : "outline"
                    text: "files"
                    onClicked: root.toggleLeft("files")
                }
                Btn {
                    kind: root.leftOpen && root.leftTab === "rooms" ? "accent" : "outline"
                    text: "rooms" + (root.openRoomName() !== "" ? " · " + root.openRoomName() : "")
                    onClicked: root.toggleLeft("rooms")
                }

                Item { Layout.fillWidth: true }

                Btn {
                    kind: root.rightOpen && root.rightTab === "interact" ? "accent" : "outline"
                    text: "interact"
                    onClicked: root.toggleRight("interact")
                }
                Btn {
                    kind: root.rightOpen && root.rightTab === "publish" ? "accent" : "outline"
                    text: "publish" + (root.pub.state !== "off" ? "  ●" : "")
                    onClicked: root.toggleRight("publish")
                }

                Rectangle {
                    implicitHeight: 30
                    implicitWidth: chainRow.implicitWidth + 24
                    radius: 3
                    color: "transparent"
                    border.color: theme.line
                    Row {
                        id: chainRow
                        anchors.centerIn: parent
                        spacing: 10
                        Rectangle { width: 8; height: 8; radius: 4; color: root.running ? theme.accent : theme.muted; anchors.verticalCenter: parent.verticalCenter }
                        Text { text: root.running ? "chain " + root.chainId : "chain stopped"; color: theme.text; font.family: root.mono; font.pixelSize: 13 }
                        Text { visible: root.running; text: root.head; color: theme.body; font.family: root.mono; font.pixelSize: 13 }
                        Text { visible: root.role !== ""; text: root.role; color: theme.cyan; font.family: root.mono; font.pixelSize: 13 }
                    }
                }

                Btn { text: "start chain"; kind: "primary"; visible: !root.running; onClicked: root.startChain() }
                Pick { id: accountBox; Layout.preferredWidth: 190; Layout.minimumWidth: 110; visible: root.running }
            }
        }

        // Body. Every divider is a drag handle: grab the grip and pull.
        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            SplitView {
                id: vSplit
                anchors.fill: parent
                anchors.margins: 8
                orientation: Qt.Vertical
                handle: Rectangle {
                    implicitHeight: 8
                    color: "transparent"
                    Rectangle { anchors.centerIn: parent; width: 40; height: 2; color: theme.lineStrong }
                }

                SplitView {
                    id: hSplit
                    SplitView.fillHeight: true
                    SplitView.minimumHeight: 260
                    orientation: Qt.Horizontal
                    handle: Rectangle {
                        implicitWidth: 8
                        color: "transparent"
                        Rectangle { anchors.centerIn: parent; width: 2; height: 40; color: theme.lineStrong }
                    }

                    // Rooms and collaboration
                    Card {
                        visible: root.leftOpen && root.leftTab === "rooms"
                        SplitView.preferredWidth: 220
                        SplitView.minimumWidth: 170
                        SplitView.maximumWidth: Math.max(240, hSplit.width * 0.4)

                        DrawerTabs {
                            id: roomTabs
                            anchors.top: parent.top
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.margins: 12
                            names: ["files", "rooms"]
                            current: "rooms"
                            onPicked: function (name) { root.leftTab = name }
                            onCollapse: root.leftOpen = false
                        }

                        ScrollView {
                            anchors.top: roomTabs.bottom
                            anchors.topMargin: 8
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.bottom: parent.bottom
                            anchors.margins: 16
                            clip: true
                            contentWidth: availableWidth

                            ColumnLayout {
                                width: parent.width
                                spacing: 10

                                Repeater {
                                    model: root.rooms
                                    delegate: Button {
                                        id: roomBtn
                                        Layout.fillWidth: true
                                        implicitHeight: 52
                                        hoverEnabled: true
                                        onClicked: root.switchRoom(index)
                                        contentItem: RowLayout {
                                            spacing: 8
                                            ColumnLayout {
                                                spacing: 0
                                                Layout.fillWidth: true
                                                Text { text: modelData.name; color: theme.text; font.pixelSize: 14; elide: Text.ElideRight; Layout.fillWidth: true }
                                                Text { text: "chain " + modelData.chainId; color: theme.muted; font.family: root.mono; font.pixelSize: 12 }
                                            }
                                            Text { text: modelData.open ? "open" : ""; color: theme.accent; font.family: root.mono; font.pixelSize: 12 }
                                        }
                                        background: Rectangle {
                                            radius: 8
                                            color: modelData.open ? theme.raised : (roomBtn.hovered ? "#121815" : "transparent")
                                            border.color: modelData.open ? theme.accent : "transparent"
                                        }
                                    }
                                }

                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 6
                                    Field { id: roomName; placeholderText: "new room"; Layout.fillWidth: true; enabled: root.running; onAccepted: root.newRoom() }
                                    Btn { text: "create"; enabled: root.running; onClicked: root.newRoom() }
                                }

                                Rectangle { Layout.fillWidth: true; Layout.topMargin: 8; Layout.bottomMargin: 4; height: 1; color: theme.line }

                                RowLayout {
                                    Layout.fillWidth: true
                                    Heading { text: "> collaborate"; Layout.fillWidth: true }
                                    Pill { visible: root.role !== ""; text: root.role; tone: theme.body; dot: false }
                                }

                                Btn {
                                    text: "share this room"
                                    visible: root.invite === ""
                                    enabled: root.running
                                    Layout.fillWidth: true
                                    onClicked: root.shareRoom()
                                }

                                RowLayout {
                                    visible: root.invite !== ""
                                    Layout.fillWidth: true
                                    spacing: 6
                                    Field { id: inviteShown; text: root.invite; readOnly: true; Layout.fillWidth: true }
                                    Btn { text: "copy"; onClicked: { inviteShown.selectAll(); inviteShown.copy(); inviteShown.deselect(); root.log("✓ invite copied") } }
                                }
                                Hint {
                                    visible: root.invite !== ""
                                    Layout.fillWidth: true
                                    text: "Anyone with this invite can join and read the room's traffic. Share it only with people you trust."
                                    color: "#E8D9AE"
                                }

                                RowLayout {
                                    Layout.fillWidth: true
                                    Layout.topMargin: 4
                                    spacing: 6
                                    Field { id: inviteInput; placeholderText: "paste an invite to join"; Layout.fillWidth: true; enabled: root.running; onAccepted: root.joinRoom() }
                                    Btn { text: "join"; enabled: root.running; onClicked: root.joinRoom() }
                                }
                            }
                        }
                    }

                    // Files: your contracts, in folders
                    Card {
                        id: filesCard
                        visible: root.leftOpen && root.leftTab === "files"
                        SplitView.preferredWidth: 220
                        SplitView.minimumWidth: 170
                        SplitView.maximumWidth: Math.max(240, hSplit.width * 0.4)

                        // The row being dragged follows the pointer.
                        Rectangle {
                            id: ghost
                            property string entry: ""
                            property string label: ""
                            visible: false
                            z: 50
                            width: ghostText.implicitWidth + 24
                            height: 30
                            radius: 3
                            color: "#1C2A22"
                            border.color: theme.accent
                            Drag.keys: ["entry"]
                            Drag.dragType: Drag.Internal
                            Drag.hotSpot.x: 10
                            Drag.hotSpot.y: 15
                            Text { id: ghostText; anchors.centerIn: parent; text: ghost.label; color: theme.text; font.family: root.mono; font.pixelSize: 13 }
                        }

                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: 12
                            spacing: 8

                            DrawerTabs {
                                Layout.fillWidth: true
                                names: ["files", "rooms"]
                                current: "files"
                                onPicked: function (name) { root.leftTab = name }
                                onCollapse: root.leftOpen = false
                            }

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 2
                                Item { Layout.fillWidth: true }
                                Btn { text: "+ file"; kind: "ghost"; implicitHeight: 30; enabled: root.running; onClicked: root.startFileAction("newfile") }
                                Btn { text: "+ folder"; kind: "ghost"; implicitHeight: 30; enabled: root.running; onClicked: root.startFileAction("newfolder") }
                            }

                            Hint {
                                Layout.fillWidth: true
                                visible: root.fileAction === "" && root.running
                                text: "New items go in: " + (root.targetDir() === "" ? "top level" : root.targetDir())
                            }
                            Hint { Layout.fillWidth: true; visible: !root.running; text: "Start the chain to use files." }

                            // new file / new folder / rename
                            ColumnLayout {
                                visible: root.fileAction === "newfile" || root.fileAction === "newfolder" || root.fileAction === "rename"
                                Layout.fillWidth: true
                                spacing: 6
                                Label {
                                    text: root.fileAction === "newfile" ? "File name (in " + (root.targetDir() === "" ? "top level" : root.targetDir()) + ")"
                                        : root.fileAction === "newfolder" ? "Folder name (in " + (root.targetDir() === "" ? "top level" : root.targetDir()) + ")"
                                        : "New name"
                                    color: theme.body
                                    font.pixelSize: 13
                                    wrapMode: Text.WordWrap
                                    Layout.fillWidth: true
                                }
                                Field {
                                    id: actionInput
                                    Layout.fillWidth: true
                                    placeholderText: root.fileAction === "newfolder" ? "tokens" : "Token.sol"
                                    onAccepted: root.runFileAction()
                                    Keys.onEscapePressed: root.fileAction = ""
                                }
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 6
                                    Btn { text: "ok"; kind: "accent"; Layout.fillWidth: true; onClicked: root.runFileAction() }
                                    Btn { text: "cancel"; Layout.fillWidth: true; onClicked: root.fileAction = "" }
                                }
                            }

                            // delete: ask first
                            ColumnLayout {
                                visible: root.fileAction === "delete"
                                Layout.fillWidth: true
                                spacing: 6
                                Label {
                                    text: "Delete " + root.selected + (root.isDir(root.selected) ? " and everything in it?" : "?")
                                    color: theme.amber
                                    font.pixelSize: 13
                                    wrapMode: Text.WrapAnywhere
                                    Layout.fillWidth: true
                                }
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 6
                                    Btn { text: "delete"; kind: "accent"; Layout.fillWidth: true; onClicked: root.runFileAction() }
                                    Btn { text: "keep"; Layout.fillWidth: true; onClicked: root.fileAction = "" }
                                }
                            }

                            Item {
                                Layout.fillWidth: true
                                Layout.fillHeight: true

                                // Empty space is the top level: drop here to move something out of a folder.
                                DropArea {
                                    id: topDrop
                                    anchors.fill: parent
                                    keys: ["entry"]
                                    onDropped: function (drop) { Qt.callLater(root.moveEntry, drop.source.entry, "") }
                                }
                                Rectangle {
                                    anchors.fill: parent
                                    radius: 3
                                    color: "transparent"
                                    border.color: topDrop.containsDrag ? theme.accent : "transparent"
                                }

                                ListView {
                                    id: fileList
                                    anchors.fill: parent
                                    anchors.margins: 2
                                    clip: true
                                    model: root.tree
                                    spacing: 1
                                    ScrollBar.vertical: ScrollBar {}

                                    delegate: Item {
                                        id: row
                                        width: fileList.width
                                        height: 32
                                        readonly property bool isSel: root.selected === modelData.path
                                        readonly property bool isOpen: root.currentFile === modelData.path

                                        Rectangle {
                                            anchors.fill: parent
                                            anchors.rightMargin: 8
                                            radius: 3
                                            color: rowDrop.containsDrag ? "#1C2A22" : (row.isSel ? theme.raised : (rowMouse.containsMouse ? "#121815" : "transparent"))
                                            border.color: rowDrop.containsDrag ? theme.accent : "transparent"
                                        }
                                        RowLayout {
                                            anchors.fill: parent
                                            anchors.leftMargin: 8 + modelData.depth * 16
                                            anchors.rightMargin: 16
                                            spacing: 6
                                            Text {
                                                text: modelData.dir ? (modelData.closed ? "▸" : "▾") : "·"
                                                color: modelData.dir ? theme.muted : theme.gutter
                                                font.pixelSize: 13
                                                Layout.preferredWidth: 12
                                            }
                                            Text {
                                                text: modelData.name
                                                color: row.isOpen ? theme.accent : (modelData.dir ? theme.body : theme.text)
                                                font.family: root.mono
                                                font.pixelSize: 13
                                                elide: Text.ElideRight
                                                Layout.fillWidth: true
                                            }
                                        }
                                        DropArea {
                                            id: rowDrop
                                            anchors.fill: parent
                                            keys: ["entry"]
                                            onDropped: function (drop) {
                                                Qt.callLater(root.moveEntry, drop.source.entry,
                                                             modelData.dir ? modelData.path : root.parentOf(modelData.path))
                                            }
                                        }
                                        MouseArea {
                                            id: rowMouse
                                            anchors.fill: parent
                                            hoverEnabled: true
                                            preventStealing: true
                                            drag.target: ghost
                                            drag.threshold: 8
                                            onPressed: function (mouse) {
                                                var p = mapToItem(ghost.parent, mouse.x, mouse.y)
                                                ghost.x = p.x - 10
                                                ghost.y = p.y - 15
                                                ghost.entry = modelData.path
                                                ghost.label = modelData.name
                                            }
                                            drag.onActiveChanged: {
                                                if (drag.active) {
                                                    ghost.visible = true
                                                    ghost.Drag.active = true
                                                } else {
                                                    // Hide first: the drop refreshes the list and
                                                    // destroys this row.
                                                    ghost.visible = false
                                                    ghost.Drag.drop()
                                                }
                                            }
                                            onClicked: root.clickEntry(modelData)
                                        }
                                    }
                                }

                                Hint {
                                    anchors.centerIn: parent
                                    width: parent.width - 20
                                    horizontalAlignment: Text.AlignHCenter
                                    visible: root.running && root.tree.length === 0
                                    text: "No files yet. Use + file."
                                }
                            }

                            RowLayout {
                                visible: root.selected !== "" && root.fileAction === ""
                                Layout.fillWidth: true
                                spacing: 6
                                Btn { text: "rename"; Layout.fillWidth: true; onClicked: root.startFileAction("rename") }
                                Btn { text: "delete"; Layout.fillWidth: true; onClicked: root.startFileAction("delete") }
                            }
                        }
                    }

                    // Editor: the main stage
                    Card {
                        id: editorCard
                        SplitView.fillWidth: true
                        SplitView.minimumWidth: 200
                        clip: true

                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: 1
                            spacing: 0

                            RowLayout {
                                Layout.fillWidth: true
                                Layout.preferredHeight: 46
                                Layout.maximumHeight: 46
                                Layout.leftMargin: 18
                                Layout.rightMargin: 18
                                spacing: 14
                                Heading { text: "write" }
                                Item {
                                    implicitWidth: tabName.implicitWidth
                                    Layout.fillHeight: true
                                    Text {
                                        id: tabName
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: root.currentFile !== "" ? root.currentFile : "untitled.sol"
                                        color: theme.text
                                        font.family: root.mono
                                        font.pixelSize: 13
                                    }
                                    Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 2; color: theme.accent }
                                }
                                Item { Layout.fillWidth: true }
                                Label {
                                    visible: root.currentFile !== ""
                                    text: root.dirty ? "● unsaved" : "✓ saved"
                                    color: root.dirty ? theme.amber : theme.muted
                                    font.family: root.mono
                                    font.pixelSize: 12
                                    Layout.rightMargin: 10
                                }
                                Label { text: "Aa"; color: theme.muted; font.family: root.mono; font.pixelSize: 12 }
                                Zoom {
                                    Layout.preferredWidth: 120
                                    from: 11
                                    to: 28
                                    stepSize: 1
                                    value: root.editorSize
                                    onMoved: root.editorSize = Math.round(value)
                                }
                                Label { text: root.editorSize + "px"; color: theme.muted; font.family: root.mono; font.pixelSize: 12; Layout.preferredWidth: 34 }
                            }
                            Rectangle { Layout.fillWidth: true; height: 1; color: theme.line }

                            ScrollView {
                                id: editorScroll
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                clip: true
                                background: Rectangle { color: theme.ground }

                                Row {
                                    Text {
                                        id: gutter
                                        width: 28 + root.editorSize * 2.2
                                        topPadding: editor.topPadding
                                        rightPadding: 14
                                        horizontalAlignment: Text.AlignRight
                                        text: root.lineNumbers(editor.lineCount)
                                        color: theme.gutter
                                        font: editor.font
                                    }
                                    TextArea {
                                        id: editor
                                        width: Math.max(implicitWidth, editorScroll.availableWidth - gutter.width)
                                        topPadding: 16
                                        bottomPadding: 16
                                        leftPadding: 0
                                        font.family: root.mono
                                        font.pixelSize: root.editorSize
                                        color: theme.text
                                        selectionColor: "#2E5C44"
                                        selectedTextColor: theme.text
                                        wrapMode: TextEdit.NoWrap
                                        selectByMouse: true
                                        background: null
                                        onTextChanged: root.edited()
                                    }
                                }
                            }

                            Rectangle { Layout.fillWidth: true; height: 1; color: theme.line }
                            RowLayout {
                                Layout.fillWidth: true
                                Layout.margins: 12
                                Layout.leftMargin: 18
                                Layout.rightMargin: 18
                                spacing: 10

                                Btn { text: "compile"; kind: "primary"; enabled: root.running; onClicked: root.compile() }
                                Pick { id: contractBox; Layout.preferredWidth: 230; onActivated: root.loadFunctions() }
                                Btn { text: "deploy"; kind: "accent"; enabled: root.running && root.contracts.length > 0; onClicked: root.deploy() }
                                Label {
                                    Layout.fillWidth: true
                                    horizontalAlignment: Text.AlignRight
                                    elide: Text.ElideMiddle
                                    text: root.deployed === "" ? "not deployed yet" : "deployed → " + root.shorten(root.deployed)
                                    color: root.deployed === "" ? theme.muted : theme.body
                                    font.family: root.mono
                                    font.pixelSize: 13
                                }
                            }
                        }

                        // Drop a .sol file (or selected text) anywhere on the editor.
                        DropArea {
                            id: dropZone
                            anchors.fill: parent
                            onDropped: function (drop) { root.handleDrop(drop) }
                        }
                        Rectangle {
                            anchors.fill: parent
                            visible: dropZone.containsDrag
                            radius: 4
                            color: "#CC070908"
                            border.color: theme.accent
                            border.width: 2
                            Text {
                                anchors.centerIn: parent
                                text: "drop a .sol file or text to load it"
                                color: theme.accent
                                font.family: root.mono
                                font.pixelSize: 15
                            }
                        }
                    }

                    // Interact / publish, one at a time
                    Card {
                        visible: root.rightOpen
                        SplitView.preferredWidth: 320
                        SplitView.minimumWidth: 250
                        SplitView.maximumWidth: Math.max(280, hSplit.width * 0.5)

                        ColumnLayout {
                            anchors.fill: parent
                            anchors.margins: 16
                            spacing: 14

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 6
                                TabBtn { text: "interact"; active: root.rightTab === "interact"; onClicked: root.rightTab = "interact" }
                                TabBtn {
                                    text: "publish"
                                    active: root.rightTab === "publish"
                                    dot: root.pub.state !== "off"
                                    dotColor: root.pubTone()
                                    onClicked: root.rightTab = "publish"
                                }
                                Item { Layout.fillWidth: true }
                                Btn { text: "›"; kind: "ghost"; implicitHeight: 30; leftPadding: 10; rightPadding: 10; onClicked: root.rightOpen = false }
                            }

                            StackLayout {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                currentIndex: root.rightTab === "interact" ? 0 : 1

                                // -- interact
                                ScrollView {
                                    clip: true
                                    contentWidth: availableWidth

                                    ColumnLayout {
                                        width: parent.width
                                        spacing: 12

                                        RowLayout {
                                            Layout.fillWidth: true
                                            Heading { text: "call your contract"; Layout.fillWidth: true }
                                            Label {
                                                visible: root.deployed !== ""
                                                text: root.contractName() + " @ " + root.shorten(root.deployed)
                                                color: theme.body
                                                font.family: root.mono
                                                font.pixelSize: 12
                                            }
                                        }
                                        Hint {
                                            Layout.fillWidth: true
                                            text: root.deployed === "" ? "Compile and deploy a contract to call it." : "Pick a function. Send writes a transaction; call reads state for free."
                                        }

                                        Flow {
                                            Layout.fillWidth: true
                                            spacing: 6
                                            visible: root.fns.length > 0
                                            Repeater {
                                                model: root.fns
                                                delegate: Button {
                                                    id: fnBtn
                                                    readonly property bool picked: fnName.text.trim() === modelData.name
                                                    implicitHeight: 32
                                                    leftPadding: 12
                                                    rightPadding: 12
                                                    hoverEnabled: true
                                                    onClicked: { fnName.text = modelData.name; fnArgs.text = "" }
                                                    contentItem: Row {
                                                        spacing: 6
                                                        Text { text: modelData.sig; color: theme.text; font.family: root.mono; font.pixelSize: 13; anchors.verticalCenter: parent.verticalCenter }
                                                        Text { text: modelData.kind; color: modelData.kind === "read" ? theme.cyan : theme.muted; font.family: root.mono; font.pixelSize: 11; anchors.verticalCenter: parent.verticalCenter }
                                                    }
                                                    background: Rectangle {
                                                        radius: 8
                                                        color: fnBtn.picked ? theme.raised : (fnBtn.hovered ? "#121815" : "transparent")
                                                        border.color: fnBtn.picked ? theme.accent : theme.lineStrong
                                                    }
                                                }
                                            }
                                        }

                                        Field { id: fnName; placeholderText: "function"; text: "increment"; Layout.fillWidth: true }
                                        Field {
                                            id: fnArgs
                                            Layout.fillWidth: true
                                            placeholderText: {
                                                var info = root.fnInfo(fnName.text)
                                                return info ? info.hint : "args, e.g. 42, \"0xabc…\""
                                            }
                                        }

                                        RowLayout {
                                            Layout.fillWidth: true
                                            spacing: 8
                                            readonly property bool isRead: {
                                                var info = root.fnInfo(fnName.text)
                                                return info !== null && info.kind === "read"
                                            }
                                            Btn { text: "send tx"; kind: parent.isRead ? "outline" : "primary"; Layout.fillWidth: true; enabled: root.deployed !== ""; onClicked: root.invoke(true) }
                                            Btn { text: "call"; kind: parent.isRead ? "primary" : "outline"; Layout.fillWidth: true; enabled: root.deployed !== ""; onClicked: root.invoke(false) }
                                        }
                                    }
                                }

                                // -- publish
                                ScrollView {
                                    clip: true
                                    contentWidth: availableWidth

                                    ColumnLayout {
                                        width: parent.width
                                        spacing: 12

                                        RowLayout {
                                            Layout.fillWidth: true
                                            spacing: 8
                                            Heading { text: "publish to bedrock"; Layout.fillWidth: true; elide: Text.ElideRight }
                                            Pill { text: root.pubTag(); tone: root.pubTone() }
                                        }
                                        Hint { text: "Optional. The local chain works without it."; Layout.fillWidth: true; visible: root.pub.state === "off" }

                                        Label {
                                            visible: root.pub.state !== "error" && !!root.pub.message
                                            text: root.pub.message || ""
                                            color: theme.body
                                            font.pixelSize: 13
                                            wrapMode: Text.WordWrap
                                            Layout.fillWidth: true
                                        }

                                        Rectangle {
                                            visible: root.pub.state === "error"
                                            Layout.fillWidth: true
                                            implicitHeight: errCol.implicitHeight + 20
                                            radius: 3
                                            color: "#1A0F0D"
                                            border.color: "#5E2A24"
                                            ColumnLayout {
                                                id: errCol
                                                anchors.fill: parent
                                                anchors.margins: 10
                                                spacing: 4
                                                Label { text: "The publisher stopped"; color: theme.red; font.bold: true }
                                                Label {
                                                    text: "err: " + (root.pub.message || "unknown")
                                                    color: "#FFC2BA"
                                                    font.family: root.mono
                                                    font.pixelSize: 12
                                                    wrapMode: Text.WrapAnywhere
                                                    Layout.fillWidth: true
                                                }
                                            }
                                        }

                                        ColumnLayout {
                                            visible: root.pubHasProgress()
                                            Layout.fillWidth: true
                                            spacing: 8
                                            RowLayout {
                                                Layout.fillWidth: true
                                                Label { text: "block " + root.pubBlock() + " / " + root.pub.head; color: theme.body; font.family: root.mono; font.pixelSize: 13; Layout.fillWidth: true }
                                                Label { text: root.pubPercent() + "%"; color: theme.body; font.family: root.mono; font.pixelSize: 13 }
                                            }
                                            Rectangle {
                                                Layout.fillWidth: true
                                                height: 6
                                                radius: 0
                                                color: theme.line
                                                Rectangle {
                                                    width: parent.width * root.pubPercent() / 100
                                                    height: parent.height
                                                    radius: 3
                                                    color: root.pubTone()
                                                }
                                            }
                                            RowLayout {
                                                Layout.fillWidth: true
                                                spacing: 8
                                                Repeater {
                                                    model: [
                                                        { label: "on chain", value: root.pub.onChain || 0 },
                                                        { label: "finalized", value: root.pub.finalized || 0 }
                                                    ]
                                                    delegate: Rectangle {
                                                        Layout.fillWidth: true
                                                        implicitHeight: 54
                                                        radius: 3
                                                        color: "transparent"
                                                        border.color: theme.line
                                                        Column {
                                                            anchors.left: parent.left
                                                            anchors.verticalCenter: parent.verticalCenter
                                                            anchors.leftMargin: 10
                                                            Text { text: modelData.label; color: theme.muted; font.family: root.mono; font.pixelSize: 12 }
                                                            Text { text: modelData.value; color: theme.text; font.family: root.mono; font.pixelSize: 16 }
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        ColumnLayout {
                                            visible: root.pub.state === "needs_funding" && !!root.pub.walletAddress
                                            Layout.fillWidth: true
                                            spacing: 6
                                            Label { text: "paying from this address · balance " + root.balanceText(); color: theme.muted; font.family: root.mono; font.pixelSize: 13; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                                            RowLayout {
                                                Layout.fillWidth: true
                                                spacing: 6
                                                Field { id: walletShown; text: root.pub.walletAddress || ""; readOnly: true; Layout.fillWidth: true }
                                                Btn { text: "copy"; onClicked: { walletShown.selectAll(); walletShown.copy(); walletShown.deselect(); root.log("✓ wallet address copied") } }
                                            }
                                        }

                                        // Every address the wallet reported, with what it said.
                                        ColumnLayout {
                                            visible: root.pub.state !== "off" && root.pub.state !== "guest" && !!root.pub.wallet && root.pub.wallet.length > 0
                                            Layout.fillWidth: true
                                            spacing: 6

                                            RowLayout {
                                                Layout.fillWidth: true
                                                Label { text: "wallet addresses"; color: theme.muted; font.family: root.mono; font.pixelSize: 13; Layout.fillWidth: true }
                                                Btn {
                                                    visible: root.pub.pinned === true
                                                    text: "automatic"
                                                    implicitHeight: 28
                                                    onClicked: root.pubUseAddress("")
                                                }
                                            }

                                            Repeater {
                                                model: root.pub.wallet || []
                                                delegate: Rectangle {
                                                    readonly property bool current: modelData.address === root.pub.walletAddress
                                                    Layout.fillWidth: true
                                                    implicitHeight: 48
                                                    color: "transparent"
                                                    border.color: current ? theme.accent : theme.line

                                                    RowLayout {
                                                        anchors.fill: parent
                                                        anchors.leftMargin: 10
                                                        anchors.rightMargin: 8
                                                        spacing: 8
                                                        ColumnLayout {
                                                            Layout.fillWidth: true
                                                            spacing: 0
                                                            Text { text: root.shorten(modelData.address); color: theme.text; font.family: root.mono; font.pixelSize: 13 }
                                                            Text {
                                                                text: modelData.balance
                                                                color: String(modelData.balance).indexOf("error") === 0 ? theme.amber : theme.muted
                                                                font.family: root.mono
                                                                font.pixelSize: 12
                                                                wrapMode: Text.WrapAnywhere
                                                                Layout.fillWidth: true
                                                            }
                                                        }
                                                        Btn {
                                                            text: current ? "in use" : "use"
                                                            kind: current ? "accent" : "outline"
                                                            implicitHeight: 30
                                                            enabled: !current
                                                            onClicked: root.pubUseAddress(modelData.address)
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        RowLayout {
                                            visible: !!root.pub.channelId && root.pub.state !== "guest"
                                            Layout.fillWidth: true
                                            spacing: 8
                                            Label { text: "channel"; color: theme.muted; font.family: root.mono; font.pixelSize: 13 }
                                            Label {
                                                text: root.pub.channelId || ""
                                                color: theme.body
                                                font.family: root.mono
                                                font.pixelSize: 13
                                                elide: Text.ElideMiddle
                                                horizontalAlignment: Text.AlignRight
                                                Layout.fillWidth: true
                                            }
                                        }

                                        Label {
                                            visible: root.pub.state === "off"
                                            text: "Sends this room's blocks to the Logos network and pays fees from your Basecamp wallet. Nothing leaves this machine until you press it."
                                            color: theme.body
                                            font.pixelSize: 13
                                            wrapMode: Text.WordWrap
                                            Layout.fillWidth: true
                                        }
                                        Flow {
                                            visible: root.pub.state === "off"
                                            Layout.fillWidth: true
                                            spacing: 8
                                            Btn { text: "publish to bedrock"; kind: "primary"; enabled: root.running; onClicked: root.pubStart() }
                                            Btn { text: root.importOpen ? "cancel import" : "use existing channel…"; enabled: root.running; onClicked: root.importOpen = !root.importOpen }
                                        }

                                        Rectangle {
                                            visible: root.importOpen && root.pub.state === "off"
                                            Layout.fillWidth: true
                                            implicitHeight: importCol.implicitHeight + 24
                                            radius: 3
                                            color: theme.ground
                                            border.color: theme.lineStrong
                                            ColumnLayout {
                                                id: importCol
                                                anchors.fill: parent
                                                anchors.margins: 12
                                                spacing: 6
                                                Label { text: "Channel id"; color: theme.body; font.pixelSize: 13 }
                                                Field { id: chanInput; placeholderText: "64 hex chars or a file path"; Layout.fillWidth: true }
                                                Label { text: "Sequencer key (secret, stays on this machine)"; color: theme.body; font.pixelSize: 13; Layout.topMargin: 4 }
                                                Field { id: keyInput; placeholderText: "64 hex chars or a file path"; echoMode: TextInput.Password; Layout.fillWidth: true }
                                                Btn { text: "use channel"; kind: "accent"; Layout.alignment: Qt.AlignRight; Layout.topMargin: 4; onClicked: root.pubImport() }
                                            }
                                        }

                                        Btn {
                                            visible: root.pub.state !== "off" && root.pub.state !== "guest"
                                            text: "stop publishing"
                                            onClicked: root.pubStop()
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Output log: the arrow opens and closes it; drag its top edge to resize.
                Rectangle {
                    id: outputCard
                    SplitView.preferredHeight: 170
                    SplitView.minimumHeight: 42
                    SplitView.maximumHeight: 460
                    radius: 4
                    color: theme.terminal
                    border.color: theme.line
                    clip: true

                    ColumnLayout {
                        anchors.fill: parent
                        anchors.margins: 1
                        spacing: 0

                        RowLayout {
                            Layout.fillWidth: true
                            Layout.fillHeight: false
                            Layout.preferredHeight: 40
                            Layout.maximumHeight: 40
                            Layout.leftMargin: 18
                            Layout.rightMargin: 8
                            spacing: 14

                            Item {
                                Layout.preferredWidth: outTitle.implicitWidth + 12
                                Layout.fillHeight: true
                                Row {
                                    id: outTitle
                                    anchors.verticalCenter: parent.verticalCenter
                                    spacing: 8
                                    Text { text: outputCard.height > 70 ? "▾" : "▸"; color: theme.accent; font.pixelSize: 15; font.bold: true; width: 12 }
                                    Heading { text: "output" }
                                    Text { visible: logModel.count > 0; text: logModel.count; color: theme.muted; font.family: root.mono; font.pixelSize: 12; anchors.verticalCenter: parent.verticalCenter }
                                }
                                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: root.toggleOutput() }
                            }

                            // Closed: show the latest line, so nothing goes unseen.
                            Label {
                                visible: outputCard.height <= 70
                                text: root.lastLine
                                color: theme.muted
                                font.family: root.mono
                                font.pixelSize: 12
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }
                            Item { visible: outputCard.height > 70; Layout.fillWidth: true }

                            Btn { text: "clear"; kind: "ghost"; implicitHeight: 30; visible: outputCard.height > 70; onClicked: logModel.clear() }
                        }
                        Rectangle { Layout.fillWidth: true; height: 1; color: "#141B17" }

                        ListView {
                            id: outputList
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            Layout.leftMargin: 18
                            Layout.rightMargin: 18
                            topMargin: 8
                            bottomMargin: 8
                            clip: true
                            model: logModel
                            spacing: 3
                            ScrollBar.vertical: ScrollBar {}
                            delegate: RowLayout {
                                width: outputList.width
                                spacing: 12
                                Text { text: tag; color: tone; font.family: root.mono; font.pixelSize: 13; Layout.preferredWidth: 42; Layout.alignment: Qt.AlignTop }
                                TextEdit {
                                    text: line
                                    readOnly: true
                                    selectByMouse: true
                                    color: theme.text
                                    selectionColor: "#2E5C44"
                                    font.family: root.mono
                                    font.pixelSize: 13
                                    wrapMode: Text.WrapAnywhere
                                    Layout.fillWidth: true
                                }
                            }
                        }
                    }
                }
            }
        }

        // Status bar
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 34
            color: theme.surface
            Rectangle { anchors.top: parent.top; width: parent.width; height: 1; color: theme.line }

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 16
                anchors.rightMargin: 16
                spacing: 10

                Rectangle { width: 8; height: 8; radius: 4; color: root.net.online ? theme.accent : theme.muted }
                Label { text: root.netText(); color: theme.body; font.family: root.mono; font.pixelSize: 12; elide: Text.ElideRight; Layout.fillWidth: true }
                Btn { text: "go offline"; visible: root.net.online === true; implicitHeight: 26; leftPadding: 8; rightPadding: 8; onClicked: root.goOffline() }

                Rectangle { width: 8; height: 8; radius: 4; color: root.pubTone(); Layout.leftMargin: 12 }
                Label { text: root.pubFooter(); color: theme.body; font.family: root.mono; font.pixelSize: 12 }
            }
        }
    }
}
