import QtQuick 2.15
import QtQuick.Controls 2.15
import QtQuick.Layouts 1.15

// SoliCamp phase 0: compile and deploy a contract on a local Solizone chain,
// all inside Basecamp. Every call goes to the solizone_module core module.
Item {
    id: root
    width: 900
    height: 640

    readonly property string mod: "solizone_module"

    property bool running: false
    property string head: "-"
    property var contracts: []
    property int contractIndex: 0
    property string deployed: ""
    property var abiCache: ({})

    readonly property string sample:
        "// SPDX-License-Identifier: MIT\n" +
        "pragma solidity ^0.8.20;\n\n" +
        "contract Counter {\n" +
        "    uint256 public count;\n\n" +
        "    function increment() public { count += 1; }\n" +
        "    function set(uint256 v) public { count = v; }\n" +
        "}\n"

    function log(text) {
        output.append(text)
    }

    // The bridge hands back the module's JSON text. Depending on the host it may
    // be wrapped in one more JSON string, so unwrap until it is an object.
    function parse(raw) {
        var value = raw
        for (var i = 0; i < 3 && typeof value === "string"; i++) {
            try { value = JSON.parse(value) } catch (e) { return { error: "unreadable reply: " + value } }
        }
        return value || { error: "empty reply" }
    }

    function call(method, args, done) {
        logos.callModuleAsync(mod, method, args, function (raw) {
            var reply = parse(raw)
            if (reply.error) {
                log("✗ " + method + ": " + (reply.error.message || reply.error))
                return
            }
            done(reply)
        })
    }

    function refresh() {
        call("status", [], function (s) {
            running = s.running === true
            head = running ? "#" + s.head : "-"
        })
    }

    function startChain() {
        // An empty directory means the module's default (~/.solicamp).
        call("start", [""], function (s) {
            running = true
            log("✓ chain " + s.chainId + (s.fresh ? " created" : " resumed") + ", head #" + s.head)
            call("accounts", [], function (a) {
                accountBox.model = a.accounts.map(function (x) { return x.index + "  " + x.address })
            })
            refresh()
        })
    }

    function compile() {
        call("compile", [editor.text], function (r) {
            contracts = r.contracts
            contractBox.model = contracts.map(function (c) { return c.name })
            contractIndex = 0
            log("✓ compiled " + contracts.length + " contract(s) with " + r.solc)
        })
    }

    function currentAbi() {
        var abi = contracts[contractBox.currentIndex].abi
        return typeof abi === "string" ? abi : JSON.stringify(abi)
    }

    function deploy() {
        var c = contracts[contractBox.currentIndex]
        call("deploy", [accountBox.currentIndex, c.bytecode, ""], function (r) {
            if (!r.success) { log("✗ deploy reverted in " + r.txHash); return }
            deployed = r.contractAddress
            log("✓ " + c.name + " deployed at " + deployed + " (block " + parseInt(r.blockNumber) + ")")
            refresh()
        })
    }

    function invoke(write) {
        if (deployed === "") { log("✗ deploy a contract first"); return }
        var name = fnName.text.trim()
        var args = "[" + fnArgs.text + "]"

        call("encode", [currentAbi(), name, args], function (e) {
            if (write) {
                call("send", [accountBox.currentIndex, deployed, e.data, "0"], function (r) {
                    log((r.success ? "✓ " : "✗ ") + name + " in block " + parseInt(r.blockNumber) + " (" + r.txHash + ")")
                    refresh()
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
        onTriggered: root.refresh()
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 8

        RowLayout {
            spacing: 8
            Button { text: running ? "Chain running" : "Start chain"; enabled: !running; onClicked: startChain() }
            Label { text: "head " + head }
            Item { Layout.fillWidth: true }
            ComboBox { id: accountBox; Layout.preferredWidth: 380; enabled: running }
        }

        ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true
            TextArea {
                id: editor
                font.family: "monospace"
                wrapMode: TextEdit.NoWrap
                selectByMouse: true
            }
        }

        RowLayout {
            spacing: 8
            Button { text: "Compile"; enabled: running; onClicked: compile() }
            ComboBox { id: contractBox; Layout.preferredWidth: 200 }
            Button { text: "Deploy"; enabled: running && contracts.length > 0; onClicked: deploy() }
            Label { text: deployed === "" ? "" : "at " + deployed; elide: Text.ElideMiddle; Layout.fillWidth: true }
        }

        RowLayout {
            spacing: 8
            TextField { id: fnName; placeholderText: "function"; text: "increment"; Layout.preferredWidth: 160 }
            TextField { id: fnArgs; placeholderText: "args, e.g. 42, \"0xabc…\""; Layout.fillWidth: true }
            Button { text: "Send"; enabled: deployed !== ""; onClicked: invoke(true) }
            Button { text: "Call"; enabled: deployed !== ""; onClicked: invoke(false) }
        }

        ScrollView {
            Layout.fillWidth: true
            Layout.preferredHeight: 150
            TextArea { id: output; readOnly: true; font.family: "monospace"; selectByMouse: true }
        }
    }
}
