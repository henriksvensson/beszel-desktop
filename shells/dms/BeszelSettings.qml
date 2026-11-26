import QtQuick
import Quickshell
import Quickshell.Io
import qs.Common
import qs.DCommon.Widgets
import qs.Modules.Plugins

PluginSettings {
    id: root
    pluginId: "beszelDesktop"

    property var discoveredSystems: []
    property bool relayConnected: false

    readonly property string resolvedSocketPath: {
        const custom = (root.pluginService && root.loadValue)
            ? String(root.loadValue("socketPath", "") || "").trim()
            : "";
        if (custom.length > 0) {
            return custom;
        }
        const runtimeDir = Quickshell.env("XDG_RUNTIME_DIR");
        return (runtimeDir && runtimeDir.length > 0)
            ? runtimeDir + "/beszel-relay.sock"
            : "/tmp/beszel-relay.sock";
    }

    function hasVariantFor(hostName, systemId) {
        const list = root.variants || [];
        const needle = String(hostName || "").toLowerCase();
        for (let i = 0; i < list.length; i++) {
            const v = list[i];
            const filter = String(v.systemFilter || v.name || "").toLowerCase();
            if (filter === needle || (systemId && filter === String(systemId).toLowerCase())) {
                return true;
            }
        }
        return false;
    }

    function addHostVariant(hostName) {
        const trimmed = String(hostName || "").trim();
        if (trimmed.length === 0 || hasVariantFor(trimmed, "")) {
            return;
        }
        root.createVariant(trimmed, {
            systemFilter: trimmed,
            description: "Live Beszel metrics for " + trimmed
        });
        root.loadVariants();
    }

    function handleRelayLine(rawLine) {
        const trimmed = String(rawLine || "").trim();
        if (trimmed.length === 0) {
            return;
        }
        let msg = null;
        try {
            msg = JSON.parse(trimmed);
        } catch (err) {
            return;
        }
        if (msg.type === "snapshot") {
            const next = [];
            const incoming = msg.systems || [];
            for (let i = 0; i < incoming.length; i++) {
                if (incoming[i] && incoming[i].system) {
                    next.push(incoming[i].system);
                }
            }
            discoveredSystems = next;
        } else if (msg.type === "systems_changed") {
            discoveredSystems = msg.systems || [];
        }
    }

    SplitParser {
        id: discoveryParser
        splitMarker: "\n"
        onRead: line => root.handleRelayLine(line)
    }

    Timer {
        id: discoveryRetryTimer
        interval: 2500
        repeat: false
        onTriggered: {
            if (!root.relayConnected) {
                discoverySocketLoader.active = false;
                discoverySocketLoader.active = true;
            }
        }
    }

    Column {
        width: parent ? parent.width : 0
        spacing: Theme.spacingS

        Loader {
            id: discoverySocketLoader
            visible: false
            width: 0
            height: 0
            active: true
            onLoaded: item.connected = true

            sourceComponent: Socket {
                path: root.resolvedSocketPath
                parser: discoveryParser

                onConnectionStateChanged: {
                    root.relayConnected = connected;
                    if (!connected) {
                        discoveryRetryTimer.restart();
                    }
                }

                onError: err => {
                    root.relayConnected = false;
                    discoveryRetryTimer.restart();
                }
            }
        }

        StyledText {
            text: "Host widgets"
            font.pixelSize: Theme.fontSizeMedium
            font.weight: Theme.fontWeightMedium
            color: Theme.surfaceText
        }

        StyledText {
            width: parent.width
            wrapMode: Text.WordWrap
            text: "Create one variant per host to place separate pills on your DankBar. Each variant appears as \"Beszel Desktop - <host>\" in the widget picker."
            font.pixelSize: Theme.fontSizeSmall
            color: Theme.surfaceVariantText
        }

        Column {
            width: parent.width
            spacing: Theme.spacingXS
            visible: root.discoveredSystems.length > 0

            StyledText {
                text: "Discovered hosts"
                font.pixelSize: Theme.fontSizeSmall
                color: Theme.surfaceVariantText
            }

            Flow {
                width: parent.width
                spacing: Theme.spacingXS

                Repeater {
                    model: root.discoveredSystems

                    Rectangle {
                        id: discoveredChip
                        required property var modelData
                        readonly property bool added: root.hasVariantFor(modelData.name, modelData.id)

                        height: discoveredRow.implicitHeight + Theme.spacingXS * 2
                        width: discoveredRow.implicitWidth + Theme.spacingM * 2
                        radius: Theme.cornerRadiusSmall
                        color: added ? Theme.surfaceContainer : Theme.primaryContainer
                        opacity: added ? 0.72 : 1.0

                        Row {
                            id: discoveredRow
                            anchors.centerIn: parent
                            spacing: Theme.spacingXS

                            Rectangle {
                                width: 6
                                height: 6
                                radius: 3
                                anchors.verticalCenter: parent.verticalCenter
                                color: discoveredChip.modelData.status === "up" ? Theme.primary : Theme.warning
                            }

                            StyledText {
                                anchors.verticalCenter: parent.verticalCenter
                                text: discoveredChip.added
                                    ? discoveredChip.modelData.name + " (added)"
                                    : "Add " + discoveredChip.modelData.name
                                font.pixelSize: Theme.fontSizeSmall
                                color: discoveredChip.added ? Theme.surfaceVariantText : Theme.primary
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            enabled: !discoveredChip.added
                            cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
                            onClicked: root.addHostVariant(discoveredChip.modelData.name)
                        }
                    }
                }
            }
        }

        StyledText {
            width: parent.width
            visible: root.discoveredSystems.length === 0
            wrapMode: Text.WordWrap
            text: root.relayConnected
                ? "Waiting for systems from beszel-relay."
                : "Start beszel-relay to discover hosts automatically, or enter a host name below."
            font.pixelSize: Theme.fontSizeSmall
            color: Theme.surfaceVariantText
        }

        Row {
            width: parent.width
            spacing: Theme.spacingS

            DTextField {
                id: hostInput
                width: Math.max(160, parent.width - addHostButton.width - Theme.spacingS)
                placeholderText: "Host name or system ID"
                Keys.onReturnPressed: addHostButton.clicked()
            }

            DButton {
                id: addHostButton
                text: "Add"
                height: 36
                onClicked: {
                    root.addHostVariant(hostInput.text);
                    hostInput.text = "";
                }
            }
        }

        Repeater {
            model: root.variants

            Rectangle {
                required property var modelData
                width: parent.width
                height: 40
                radius: Theme.cornerRadiusSmall
                color: Theme.surfaceContainerHigh

                StyledText {
                    anchors.left: parent.left
                    anchors.leftMargin: Theme.spacingM
                    anchors.verticalCenter: parent.verticalCenter
                    text: modelData.name + (modelData.systemFilter && modelData.systemFilter !== modelData.name ? " (" + modelData.systemFilter + ")" : "")
                    font.pixelSize: Theme.fontSizeMedium
                    color: Theme.surfaceText
                }

                DButton {
                    anchors.right: parent.right
                    anchors.rightMargin: Theme.spacingXS
                    anchors.verticalCenter: parent.verticalCenter
                    text: "Remove"
                    height: 30
                    onClicked: {
                        root.removeVariant(modelData.id);
                        root.loadVariants();
                    }
                }
            }
        }
    }

    StringSetting {
        settingKey: "warnThreshold"
        label: "Warning threshold (%)"
        description: "Highlight CPU, memory, and core bars in amber at or above this percentage."
        placeholder: "70"
        defaultValue: "70"
    }

    StringSetting {
        settingKey: "dangerThreshold"
        label: "Danger threshold (%)"
        description: "Highlight CPU, memory, and core bars in red at or above this percentage."
        placeholder: "90"
        defaultValue: "90"
    }

    StringSetting {
        settingKey: "systemFilter"
        label: "Default system filter"
        description: "Used when no host variants are configured. Leave empty for the first subscribed system."
        placeholder: "workstation"
        defaultValue: ""
    }

    StringSetting {
        settingKey: "socketPath"
        label: "Relay socket path"
        description: "Optional override for the beszel-relay Unix socket. Defaults to $XDG_RUNTIME_DIR/beszel-relay.sock."
        placeholder: "/run/user/1000/beszel-relay.sock"
        defaultValue: ""
    }

    StringSetting {
        settingKey: "staleSeconds"
        label: "Stale threshold (seconds)"
        description: "Mark live data stale when no sample arrives within this many seconds."
        placeholder: "5"
        defaultValue: "5"
    }
}
