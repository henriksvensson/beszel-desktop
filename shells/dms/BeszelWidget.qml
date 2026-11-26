import QtQuick
import Quickshell
import Quickshell.Io
import qs.Common
import qs.DCommon.Widgets
import qs.Modules.Plugins

PluginComponent {
    id: root

    layerNamespacePlugin: "beszel-desktop"
    popoutWidth: 380

    property string variantId: ""
    property var variantData: ({})

    readonly property string configuredSocket: String(pluginData.socketPath || "").trim()
    readonly property string configuredSystem: {
        if (variantId && variantId.length > 0) {
            const vFilter = variantData && (variantData.systemFilter || variantData.name);
            return String(vFilter || "").trim();
        }
        return String(pluginData.systemFilter || "").trim();
    }
    readonly property int staleSeconds: Math.max(2, Number(pluginData.staleSeconds || 5))
    readonly property int warnThreshold: Math.max(10, Math.min(99, Number(pluginData.warnThreshold || 70)))
    readonly property int dangerThreshold: Math.max(warnThreshold + 1, Math.min(100, Number(pluginData.dangerThreshold || 90)))

    Connections {
        target: root.pluginService
        enabled: root.pluginService !== null && root.variantId.length > 0
        function onPluginDataChanged(changedPluginId) {
            if (changedPluginId === root.pluginId && root.pluginService?.getPluginVariantData) {
                root.variantData = root.pluginService.getPluginVariantData(root.pluginId, root.variantId) || {};
            }
        }
    }

    readonly property string resolvedSocketPath: {
        if (configuredSocket.length > 0) {
            return configuredSocket;
        }
        const runtimeDir = Quickshell.env("XDG_RUNTIME_DIR");
        return (runtimeDir && runtimeDir.length > 0)
            ? runtimeDir + "/beszel-relay.sock"
            : "/tmp/beszel-relay.sock";
    }

    property bool socketConnected: false
    property var connectionInfo: null
    property var systemsList: []
    property var historyBySystem: ({})
    property int historyCapacity: 60
    property double nowMs: Date.now()
    property string popoutSystemId: ""

    readonly property var barSystem: findSystem(configuredSystem)
    readonly property var barSamples: barSystem ? (historyBySystem[barSystem.id] || []) : []
    readonly property var barLatest: barSamples.length > 0 ? barSamples[barSamples.length - 1] : null

    readonly property var popoutSystem: {
        const chosen = popoutSystemId.length > 0 ? findSystemExact(popoutSystemId) : null;
        return chosen || barSystem || (systemsList.length > 0 ? systemsList[0] : null);
    }
    readonly property var popoutSamples: popoutSystem ? (historyBySystem[popoutSystem.id] || []) : []
    readonly property var popoutLatest: popoutSamples.length > 0 ? popoutSamples[popoutSamples.length - 1] : null
    readonly property var popoutCores: (popoutLatest && popoutLatest.cpu_cores) ? popoutLatest.cpu_cores : []

    readonly property bool isStale: {
        if (!barLatest || !barLatest.timestamp_ms) {
            return false;
        }
        return (nowMs - Number(barLatest.timestamp_ms)) > (staleSeconds * 1000);
    }

    readonly property string effectiveState: {
        if (!socketConnected || !connectionInfo) {
            return "disconnected";
        }
        if (connectionInfo.state !== "connected") {
            return connectionInfo.state;
        }
        if (configuredSystem.length > 0 && !barSystem) {
            return "not_found";
        }
        return isStale ? "stale" : "connected";
    }

    readonly property color statusColor: {
        if (effectiveState === "connected") {
            return Theme.primary;
        }
        if (effectiveState === "stale" || effectiveState === "connecting"
            || effectiveState === "reconnecting" || effectiveState === "not_found"
            || effectiveState === "paused") {
            return Theme.warning;
        }
        return Theme.error;
    }

    onConfiguredSystemChanged: popoutSystemId = ""

    function severityColor(pct, normalColor) {
        const value = Number(pct || 0);
        if (value >= dangerThreshold) {
            return Theme.error;
        }
        if (value >= warnThreshold) {
            return Theme.warning;
        }
        return normalColor;
    }

    function hostPeakColor(systemId) {
        const samples = historyBySystem[systemId];
        if (!samples || samples.length === 0) {
            return Theme.surfaceVariantText;
        }
        const latest = samples[samples.length - 1];
        const peak = Math.max(Number(latest.cpu_pct || 0), Number(latest.mem_pct || 0));
        return severityColor(peak, Theme.primary);
    }

    function findSystemExact(targetId) {
        if (!systemsList || !targetId) {
            return null;
        }
        for (let i = 0; i < systemsList.length; i++) {
            if (systemsList[i].id === targetId) {
                return systemsList[i];
            }
        }
        return null;
    }

    function findSystem(filter) {
        if (!systemsList || systemsList.length === 0) {
            return null;
        }
        if (!filter || filter.length === 0) {
            return systemsList[0];
        }
        const needle = filter.toLowerCase();
        for (let i = 0; i < systemsList.length; i++) {
            const s = systemsList[i];
            if (s.id === filter || String(s.name || "").toLowerCase() === needle) {
                return s;
            }
        }
        return null;
    }

    function formatPercent(value) {
        const n = Math.max(0, Math.round(Number(value || 0)));
        return (n < 10 ? "0" + n : String(n)) + "%";
    }

    function formatGiB(bytes) {
        const gib = Number(bytes) / (1024 * 1024 * 1024);
        return gib.toFixed(1);
    }

    function stateDisplayLabel(state) {
        switch (state) {
        case "connected":
            return "Live";
        case "paused":
            return "Paused";
        case "stale":
            return "Stale";
        case "connecting":
            return "Connecting";
        case "reconnecting":
            return "Reconnecting";
        case "not_found":
            return configuredSystem + " not found";
        case "auth_error":
            return "Authentication error";
        default:
            return "Relay offline";
        }
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
            console.warn("beszel-desktop: invalid NDJSON line:", err);
            return;
        }

        if (msg.type === "snapshot") {
            applySnapshot(msg);
        } else if (msg.type === "sample") {
            applySample(msg.system_id, msg.sample);
        } else if (msg.type === "connection") {
            connectionInfo = msg.connection;
        } else if (msg.type === "systems_changed") {
            systemsList = msg.systems || [];
        }
    }

    function applySnapshot(msg) {
        connectionInfo = msg.connection || null;
        const incoming = msg.systems || [];
        const nextSystems = [];
        const nextHistory = {};
        let cap = historyCapacity;

        for (let i = 0; i < incoming.length; i++) {
            const entry = incoming[i];
            if (!entry || !entry.system) {
                continue;
            }
            nextSystems.push(entry.system);
            nextHistory[entry.system.id] = entry.samples ? entry.samples.slice() : [];
            if (entry.history_capacity) {
                cap = Number(entry.history_capacity);
            }
        }

        historyCapacity = Math.max(1, cap);
        systemsList = nextSystems;
        historyBySystem = nextHistory;
        nowMs = Date.now();
    }

    function applySample(systemId, sample) {
        if (!systemId || !sample) {
            return;
        }
        const nextHistory = Object.assign({}, historyBySystem);
        const existing = nextHistory[systemId] ? nextHistory[systemId].slice() : [];
        existing.push(sample);
        while (existing.length > historyCapacity) {
            existing.shift();
        }
        nextHistory[systemId] = existing;
        historyBySystem = nextHistory;
        nowMs = Date.now();
    }

    function restartSocket() {
        socketLoader.active = false;
        root.socketConnected = false;
        socketLoader.active = true;
    }

    SplitParser {
        id: relayParser
        splitMarker: "\n"
        onRead: line => root.handleRelayLine(line)
    }

    Loader {
        id: socketLoader
        active: true

        onLoaded: item.connected = true

        sourceComponent: Socket {
            path: root.resolvedSocketPath
            parser: relayParser

            onConnectionStateChanged: {
                root.socketConnected = connected;
                if (!connected) {
                    reconnectTimer.restart();
                }
            }

            onError: err => {
                root.socketConnected = false;
                reconnectTimer.restart();
            }
        }
    }

    onResolvedSocketPathChanged: restartSocket()

    Timer {
        id: reconnectTimer
        interval: 2000
        repeat: false
        onTriggered: {
            if (!root.socketConnected) {
                root.restartSocket();
            }
        }
    }

    Timer {
        id: stalenessTimer
        interval: 1000
        repeat: true
        running: true
        onTriggered: root.nowMs = Date.now()
    }

    component MetricSparkline: Item {
        id: spark

        property var samples: []
        property string valueKey: "cpu_pct"
        property color lineColor: Theme.primary
        property int historyCapacity: 60
        property int gapThresholdMs: 3500

        property bool hovering: false
        property int hoveredIndex: -1
        readonly property var hoveredSample: (hovering && hoveredIndex >= 0 && samples && hoveredIndex < samples.length)
            ? samples[hoveredIndex]
            : null
        readonly property real hoveredValue: hoveredSample ? Number(hoveredSample[valueKey]) : -1

        readonly property real latestValue: {
            const pts = samples || [];
            if (pts.length === 0) {
                return -1;
            }
            const v = Number(pts[pts.length - 1][valueKey]);
            return isFinite(v) ? Math.max(0, Math.min(100, v)) : -1;
        }

        property real animatedLatestValue: latestValue >= 0 ? latestValue : 0

        Behavior on animatedLatestValue {
            NumberAnimation {
                duration: 240
                easing.type: Easing.OutCubic
            }
        }

        Behavior on lineColor {
            ColorAnimation {
                duration: 240
                easing.type: Easing.OutCubic
            }
        }

        implicitHeight: 68

        onSamplesChanged: sparkCanvas.requestPaint()
        onLineColorChanged: sparkCanvas.requestPaint()
        onAnimatedLatestValueChanged: sparkCanvas.requestPaint()
        onHoveredIndexChanged: sparkCanvas.requestPaint()
        onHoveringChanged: sparkCanvas.requestPaint()

        Rectangle {
            anchors.fill: parent
            radius: Theme.cornerRadiusSmall
            color: Theme.surfaceContainerHigh
        }

        Canvas {
            id: sparkCanvas
            anchors.fill: parent
            anchors.margins: Theme.spacingXS

            onWidthChanged: requestPaint()
            onHeightChanged: requestPaint()

            function yForPct(pct) {
                const clamped = Math.max(0, Math.min(100, pct));
                const topPad = 5;
                const bottomPad = 4;
                const usable = Math.max(1, height - topPad - bottomPad);
                return topPad + usable * (1.0 - clamped / 100.0);
            }

            function xForIndex(index, totalCount) {
                const cap = Math.max(2, spark.historyCapacity, totalCount);
                const stepX = width / (cap - 1);
                const offset = cap - totalCount;
                return (offset + index) * stepX;
            }

            function drawGrid(ctx) {
                ctx.save();
                ctx.strokeStyle = Theme.withAlpha(Theme.surfaceText, 0.08);
                ctx.lineWidth = 1;
                const marks = [25, 50, 75];
                for (let i = 0; i < marks.length; i++) {
                    const y = Math.round(yForPct(marks[i])) + 0.5;
                    ctx.beginPath();
                    ctx.moveTo(0, y);
                    ctx.lineTo(width, y);
                    ctx.stroke();
                }
                ctx.restore();
            }

            function traceSmoothPath(ctx, seg) {
                if (seg.length === 1) {
                    return;
                }
                for (let i = 1; i < seg.length - 1; i++) {
                    const midX = (seg[i].x + seg[i + 1].x) * 0.5;
                    const midY = (seg[i].y + seg[i + 1].y) * 0.5;
                    ctx.quadraticCurveTo(seg[i].x, seg[i].y, midX, midY);
                }
                const last = seg[seg.length - 1];
                ctx.lineTo(last.x, last.y);
            }

            function drawSegment(ctx, seg) {
                if (seg.length === 0) {
                    return;
                }
                const baselineY = height - 1;
                const grad = ctx.createLinearGradient(0, 0, 0, height);
                grad.addColorStop(0.0, Theme.withAlpha(spark.lineColor, 0.24));
                grad.addColorStop(1.0, Theme.withAlpha(spark.lineColor, 0.0));

                ctx.beginPath();
                ctx.moveTo(seg[0].x, baselineY);
                ctx.lineTo(seg[0].x, seg[0].y);
                traceSmoothPath(ctx, seg);
                ctx.lineTo(seg[seg.length - 1].x, baselineY);
                ctx.closePath();
                ctx.fillStyle = grad;
                ctx.fill();

                ctx.beginPath();
                ctx.moveTo(seg[0].x, seg[0].y);
                traceSmoothPath(ctx, seg);
                ctx.strokeStyle = spark.lineColor;
                ctx.lineWidth = 2;
                ctx.lineJoin = "round";
                ctx.lineCap = "round";
                ctx.stroke();
            }

            function drawPointMarker(ctx, x, y, haloRadius, coreRadius) {
                ctx.save();
                ctx.beginPath();
                ctx.arc(x, y, haloRadius, 0, Math.PI * 2);
                ctx.fillStyle = Theme.withAlpha(spark.lineColor, 0.24);
                ctx.fill();

                ctx.beginPath();
                ctx.arc(x, y, coreRadius, 0, Math.PI * 2);
                ctx.fillStyle = spark.lineColor;
                ctx.fill();
                ctx.restore();
            }

            onPaint: {
                const ctx = getContext("2d");
                ctx.reset();
                if (width <= 4 || height <= 4) {
                    return;
                }

                drawGrid(ctx);

                const pts = spark.samples || [];
                if (pts.length === 0) {
                    return;
                }

                const segments = [];
                let current = [];
                let prevTs = 0;

                for (let i = 0; i < pts.length; i++) {
                    const sample = pts[i];
                    const rawVal = Number(sample[spark.valueKey]);
                    if (!isFinite(rawVal)) {
                        if (current.length > 0) {
                            segments.push(current);
                            current = [];
                        }
                        prevTs = 0;
                        continue;
                    }

                    const ts = Number(sample.timestamp_ms || 0);
                    if (current.length > 0 && prevTs > 0 && ts - prevTs > spark.gapThresholdMs) {
                        segments.push(current);
                        current = [];
                    }

                    const isLast = i === pts.length - 1;
                    const val = isLast ? spark.animatedLatestValue : rawVal;
                    current.push({
                        x: xForIndex(i, pts.length),
                        y: yForPct(val)
                    });
                    prevTs = ts;
                }
                if (current.length > 0) {
                    segments.push(current);
                }

                for (let s = 0; s < segments.length; s++) {
                    drawSegment(ctx, segments[s]);
                }

                if (segments.length > 0) {
                    const lastSeg = segments[segments.length - 1];
                    const tip = lastSeg[lastSeg.length - 1];
                    drawPointMarker(ctx, tip.x, tip.y, 5.5, 2.6);
                }

                if (spark.hovering && spark.hoveredIndex >= 0 && spark.hoveredIndex < pts.length) {
                    const hx = xForIndex(spark.hoveredIndex, pts.length);
                    const hv = Number(pts[spark.hoveredIndex][spark.valueKey]);
                    if (isFinite(hv)) {
                        const hy = yForPct(hv);
                        ctx.save();
                        ctx.strokeStyle = Theme.withAlpha(Theme.surfaceText, 0.25);
                        ctx.lineWidth = 1;
                        ctx.beginPath();
                        ctx.moveTo(Math.round(hx) + 0.5, 0);
                        ctx.lineTo(Math.round(hx) + 0.5, height);
                        ctx.stroke();
                        ctx.restore();
                        drawPointMarker(ctx, hx, hy, 6.5, 3.2);
                    }
                }
            }
        }

        MouseArea {
            anchors.fill: sparkCanvas
            hoverEnabled: true

            function updateHover(mouseX) {
                const pts = spark.samples || [];
                if (pts.length === 0 || width <= 0) {
                    spark.hoveredIndex = -1;
                    return;
                }
                const cap = Math.max(2, spark.historyCapacity, pts.length);
                const offset = cap - pts.length;
                const slot = Math.round((mouseX / width) * (cap - 1)) - offset;
                spark.hoveredIndex = (slot >= 0 && slot < pts.length) ? slot : -1;
            }

            onEntered: {
                spark.hovering = true;
                updateHover(mouseX);
            }
            onPositionChanged: mouse => updateHover(mouse.x)
            onExited: {
                spark.hovering = false;
                spark.hoveredIndex = -1;
            }
        }
    }

    component CpuCoreHeatmap: Column {
        id: heatmap

        property var cores: []
        property int warnThreshold: 70
        property int dangerThreshold: 90
        property int hoveredCore: -1

        readonly property int coreCount: cores ? cores.length : 0
        readonly property int columnCount: {
            if (coreCount <= 8) {
                return Math.max(1, coreCount);
            }
            if (coreCount <= 16) {
                return 8;
            }
            if (coreCount <= 24) {
                return 12;
            }
            return 16;
        }

        readonly property int peakLoad: {
            if (!cores || cores.length === 0) {
                return 0;
            }
            let maxVal = 0;
            for (let i = 0; i < cores.length; i++) {
                const v = Number(cores[i] || 0);
                if (v > maxVal) {
                    maxVal = v;
                }
            }
            return Math.round(maxVal);
        }

        function coreColor(pct) {
            if (pct >= dangerThreshold) {
                return Theme.error;
            }
            if (pct >= warnThreshold) {
                return Theme.warning;
            }
            if (pct <= 4) {
                return Theme.withAlpha(Theme.primary, 0.22);
            }
            const ratio = Math.min(1.0, Math.max(0.0, pct / Math.max(1, warnThreshold)));
            return Theme.withAlpha(Theme.primary, 0.42 + 0.58 * ratio);
        }

        spacing: Theme.spacingXS

        Item {
            width: parent.width
            height: Math.max(coresTitle.implicitHeight, coresMeta.implicitHeight)

            StyledText {
                id: coresTitle
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                text: "Cores"
                font.pixelSize: Theme.fontSizeMedium
                color: Theme.surfaceText
            }

            StyledText {
                id: coresMeta
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                font.pixelSize: Theme.fontSizeSmall
                color: heatmap.hoveredCore >= 0 ? Theme.surfaceText : Theme.surfaceVariantText
                text: {
                    if (heatmap.hoveredCore >= 0 && heatmap.hoveredCore < heatmap.coreCount) {
                        const v = Math.round(Number(heatmap.cores[heatmap.hoveredCore] || 0));
                        return "Core " + heatmap.hoveredCore + "  " + v + "%";
                    }
                    return heatmap.coreCount + " cores  Peak " + heatmap.peakLoad + "%";
                }
            }
        }

        Rectangle {
            width: parent.width
            height: coreGrid.implicitHeight + Theme.spacingS * 2
            radius: Theme.cornerRadiusSmall
            color: Theme.surfaceContainerHigh

            Grid {
                id: coreGrid
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                anchors.leftMargin: Theme.spacingS
                anchors.rightMargin: Theme.spacingS
                columns: heatmap.columnCount
                columnSpacing: 4
                rowSpacing: 4

                readonly property real cellWidth: Math.max(
                    8,
                    (width - columnSpacing * Math.max(0, columns - 1)) / Math.max(1, columns)
                )

                Repeater {
                    model: heatmap.coreCount

                    Rectangle {
                        id: coreTile
                        required property int index
                        readonly property real loadPct: Math.max(
                            0,
                            Math.min(100, Number((heatmap.cores && heatmap.cores[index]) || 0))
                        )
                        readonly property bool isHovered: heatmap.hoveredCore === index

                        width: coreGrid.cellWidth
                        height: 22
                        radius: 3
                        color: Theme.surfaceContainerHighest
                        border.width: isHovered ? 1 : 0
                        border.color: Theme.withAlpha(Theme.surfaceText, 0.45)
                        clip: true

                        Rectangle {
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.bottom: parent.bottom
                            radius: 2
                            height: coreTile.loadPct > 0
                                ? Math.max(3, Math.round((coreTile.loadPct / 100.0) * coreTile.height))
                                : 0
                            color: heatmap.coreColor(coreTile.loadPct)

                            Behavior on height {
                                NumberAnimation {
                                    duration: 260
                                    easing.type: Easing.OutCubic
                                }
                            }

                            Behavior on color {
                                ColorAnimation {
                                    duration: 260
                                    easing.type: Easing.OutCubic
                                }
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            hoverEnabled: true
                            onEntered: heatmap.hoveredCore = coreTile.index
                            onExited: {
                                if (heatmap.hoveredCore === coreTile.index) {
                                    heatmap.hoveredCore = -1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    horizontalBarPill: Component {
        Row {
            id: hRow
            spacing: Theme.spacingS

            readonly property bool showMetrics: !!root.barLatest
                && root.effectiveState !== "disconnected"
                && root.effectiveState !== "auth_error"
                && root.effectiveState !== "not_found"

            readonly property color cpuIconColor: root.effectiveState === "connected"
                ? root.severityColor(root.barLatest ? root.barLatest.cpu_pct : 0, Theme.primary)
                : root.statusColor

            readonly property color cpuTextColor: root.effectiveState === "connected"
                ? root.severityColor(root.barLatest ? root.barLatest.cpu_pct : 0, Theme.widgetTextColor)
                : root.statusColor

            readonly property color memIconColor: root.effectiveState === "connected"
                ? root.severityColor(root.barLatest ? root.barLatest.mem_pct : 0, Theme.secondary)
                : root.statusColor

            readonly property color memTextColor: root.effectiveState === "connected"
                ? root.severityColor(root.barLatest ? root.barLatest.mem_pct : 0, Theme.widgetTextColor)
                : root.statusColor

            TextMetrics {
                id: pctMetrics
                font.family: Theme.fontFamily
                font.pixelSize: Theme.fontSizeSmall
                text: "99%"
            }

            StyledText {
                visible: !hRow.showMetrics
                anchors.verticalCenter: parent.verticalCenter
                font.pixelSize: Theme.fontSizeSmall
                color: root.statusColor
                text: root.stateDisplayLabel(root.effectiveState)
            }

            StyledText {
                visible: hRow.showMetrics && !!root.barSystem
                anchors.verticalCenter: parent.verticalCenter
                font.pixelSize: Theme.fontSizeSmall
                color: root.effectiveState === "connected" ? Theme.widgetTextColor : root.statusColor
                text: root.barSystem ? root.barSystem.name : ""
            }

            Row {
                visible: hRow.showMetrics
                spacing: 2
                anchors.verticalCenter: parent.verticalCenter

                DIcon {
                    name: "memory"
                    size: Theme.fontSizeMedium
                    color: hRow.cpuIconColor
                    anchors.verticalCenter: parent.verticalCenter

                    Behavior on color {
                        ColorAnimation {
                            duration: 240
                            easing.type: Easing.OutCubic
                        }
                    }
                }

                StyledText {
                    anchors.verticalCenter: parent.verticalCenter
                    width: Math.max(Math.ceil(pctMetrics.advanceWidth), implicitWidth)
                    horizontalAlignment: Text.AlignRight
                    font.pixelSize: Theme.fontSizeSmall
                    color: hRow.cpuTextColor
                    text: root.barLatest ? root.formatPercent(root.barLatest.cpu_pct) : "--%"

                    Behavior on color {
                        ColorAnimation {
                            duration: 240
                            easing.type: Easing.OutCubic
                        }
                    }
                }
            }

            Row {
                visible: hRow.showMetrics
                spacing: 2
                anchors.verticalCenter: parent.verticalCenter

                DIcon {
                    name: "developer_board"
                    size: Theme.fontSizeMedium
                    color: hRow.memIconColor
                    anchors.verticalCenter: parent.verticalCenter

                    Behavior on color {
                        ColorAnimation {
                            duration: 240
                            easing.type: Easing.OutCubic
                        }
                    }
                }

                StyledText {
                    anchors.verticalCenter: parent.verticalCenter
                    width: Math.max(Math.ceil(pctMetrics.advanceWidth), implicitWidth)
                    horizontalAlignment: Text.AlignRight
                    font.pixelSize: Theme.fontSizeSmall
                    color: hRow.memTextColor
                    text: root.barLatest ? root.formatPercent(root.barLatest.mem_pct) : "--%"

                    Behavior on color {
                        ColorAnimation {
                            duration: 240
                            easing.type: Easing.OutCubic
                        }
                    }
                }
            }
        }
    }

    verticalBarPill: Component {
        Column {
            spacing: 2

            TextMetrics {
                id: verticalPctMetrics
                font.family: Theme.fontFamily
                font.pixelSize: Theme.fontSizeSmall - 2
                text: "99%"
            }

            StyledText {
                anchors.horizontalCenter: parent.horizontalCenter
                width: Math.max(Math.ceil(verticalPctMetrics.advanceWidth), implicitWidth)
                horizontalAlignment: Text.AlignHCenter
                font.pixelSize: Theme.fontSizeSmall - 2
                color: root.effectiveState === "connected"
                    ? root.severityColor(root.barLatest ? root.barLatest.cpu_pct : 0, Theme.widgetTextColor)
                    : root.statusColor
                text: root.barLatest ? root.formatPercent(root.barLatest.cpu_pct) : "--"

                Behavior on color {
                    ColorAnimation {
                        duration: 240
                        easing.type: Easing.OutCubic
                    }
                }
            }

            StyledText {
                anchors.horizontalCenter: parent.horizontalCenter
                width: Math.max(Math.ceil(verticalPctMetrics.advanceWidth), implicitWidth)
                horizontalAlignment: Text.AlignHCenter
                font.pixelSize: Theme.fontSizeSmall - 2
                color: root.effectiveState === "connected"
                    ? root.severityColor(root.barLatest ? root.barLatest.mem_pct : 0, Theme.surfaceVariantText)
                    : root.statusColor
                text: root.barLatest ? root.formatPercent(root.barLatest.mem_pct) : "--"

                Behavior on color {
                    ColorAnimation {
                        duration: 240
                        easing.type: Easing.OutCubic
                    }
                }
            }
        }
    }

    popoutContent: Component {
        PopoutComponent {
            headerText: root.popoutSystem ? root.popoutSystem.name : "Beszel Desktop"
            detailsText: {
                if (root.connectionInfo && root.connectionInfo.last_error) {
                    return root.stateDisplayLabel(root.effectiveState) + ": " + root.connectionInfo.last_error;
                }
                if (root.effectiveState !== "connected") {
                    return root.stateDisplayLabel(root.effectiveState);
                }
                return "";
            }

            Column {
                width: parent.width
                spacing: Theme.spacingM
                leftPadding: Theme.spacingS
                rightPadding: Theme.spacingS
                bottomPadding: Theme.spacingS

                Flow {
                    width: parent.width - Theme.spacingS * 2
                    spacing: Theme.spacingXS
                    visible: root.systemsList.length > 1

                    Repeater {
                        model: root.systemsList

                        Rectangle {
                            id: hostChip
                            required property var modelData
                            readonly property bool selected: root.popoutSystem && root.popoutSystem.id === modelData.id

                            height: hostChipRow.implicitHeight + Theme.spacingXS * 2
                            width: hostChipRow.implicitWidth + Theme.spacingM * 2
                            radius: Theme.cornerRadiusSmall
                            color: selected ? Theme.primaryContainer : Theme.surfaceContainerHigh

                            Behavior on color {
                                ColorAnimation {
                                    duration: 180
                                    easing.type: Easing.OutCubic
                                }
                            }

                            Row {
                                id: hostChipRow
                                anchors.centerIn: parent
                                spacing: Theme.spacingXS

                                Rectangle {
                                    width: 6
                                    height: 6
                                    radius: 3
                                    anchors.verticalCenter: parent.verticalCenter
                                    color: root.hostPeakColor(hostChip.modelData.id)

                                    Behavior on color {
                                        ColorAnimation {
                                            duration: 240
                                            easing.type: Easing.OutCubic
                                        }
                                    }
                                }

                                StyledText {
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: hostChip.modelData.name
                                    font.pixelSize: Theme.fontSizeSmall
                                    font.weight: hostChip.selected ? Font.DemiBold : Font.Normal
                                    color: hostChip.selected ? Theme.primary : Theme.surfaceText
                                }
                            }

                            MouseArea {
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                onClicked: root.popoutSystemId = hostChip.modelData.id
                            }
                        }
                    }
                }

                Column {
                    id: cpuSection
                    width: parent.width - Theme.spacingS * 2
                    spacing: Theme.spacingXS

                    readonly property real displayCpu: cpuSpark.hovering && cpuSpark.hoveredValue >= 0
                        ? cpuSpark.hoveredValue
                        : (root.popoutLatest ? Number(root.popoutLatest.cpu_pct) : -1)

                    readonly property color cpuAccent: root.severityColor(
                        displayCpu >= 0 ? displayCpu : 0,
                        Theme.primary
                    )

                    Item {
                        width: parent.width
                        height: Math.max(cpuLabel.implicitHeight, cpuValue.implicitHeight)

                        StyledText {
                            id: cpuLabel
                            anchors.left: parent.left
                            anchors.verticalCenter: parent.verticalCenter
                            text: "CPU"
                            font.pixelSize: Theme.fontSizeMedium
                            color: Theme.surfaceText
                        }

                        StyledText {
                            id: cpuValue
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            text: cpuSection.displayCpu >= 0
                                ? cpuSection.displayCpu.toFixed(1) + "%"
                                : "--"
                            font.pixelSize: Theme.fontSizeLarge
                            font.weight: Font.DemiBold
                            color: cpuSection.cpuAccent

                            Behavior on color {
                                ColorAnimation {
                                    duration: 240
                                    easing.type: Easing.OutCubic
                                }
                            }
                        }
                    }

                    MetricSparkline {
                        id: cpuSpark
                        width: parent.width
                        samples: root.popoutSamples
                        valueKey: "cpu_pct"
                        lineColor: cpuSection.cpuAccent
                        historyCapacity: root.historyCapacity
                    }
                }

                CpuCoreHeatmap {
                    width: parent.width - Theme.spacingS * 2
                    visible: root.popoutCores.length > 0
                    cores: root.popoutCores
                    warnThreshold: root.warnThreshold
                    dangerThreshold: root.dangerThreshold
                }

                Column {
                    id: memSection
                    width: parent.width - Theme.spacingS * 2
                    spacing: Theme.spacingXS

                    readonly property real displayMem: memSpark.hovering && memSpark.hoveredValue >= 0
                        ? memSpark.hoveredValue
                        : (root.popoutLatest ? Number(root.popoutLatest.mem_pct) : -1)

                    readonly property var displaySample: memSpark.hovering && memSpark.hoveredSample
                        ? memSpark.hoveredSample
                        : root.popoutLatest

                    readonly property color memAccent: root.severityColor(
                        displayMem >= 0 ? displayMem : 0,
                        Theme.secondary
                    )

                    Item {
                        width: parent.width
                        height: Math.max(memLabel.implicitHeight, memReadoutRow.implicitHeight)

                        StyledText {
                            id: memLabel
                            anchors.left: parent.left
                            anchors.verticalCenter: parent.verticalCenter
                            text: "Memory"
                            font.pixelSize: Theme.fontSizeMedium
                            color: Theme.surfaceText
                        }

                        Row {
                            id: memReadoutRow
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: Theme.spacingS

                            StyledText {
                                anchors.verticalCenter: parent.verticalCenter
                                font.pixelSize: Theme.fontSizeSmall
                                color: Theme.surfaceVariantText
                                visible: !!(memSection.displaySample
                                    && memSection.displaySample.mem_used_bytes
                                    && memSection.displaySample.mem_total_bytes)
                                text: {
                                    const s = memSection.displaySample;
                                    if (!s || !s.mem_used_bytes || !s.mem_total_bytes) {
                                        return "";
                                    }
                                    return root.formatGiB(s.mem_used_bytes) + " / " + root.formatGiB(s.mem_total_bytes) + " GiB";
                                }
                            }

                            StyledText {
                                id: memPctText
                                anchors.verticalCenter: parent.verticalCenter
                                font.pixelSize: Theme.fontSizeLarge
                                font.weight: Font.DemiBold
                                color: memSection.memAccent
                                text: memSection.displayMem >= 0
                                    ? memSection.displayMem.toFixed(1) + "%"
                                    : "--"

                                Behavior on color {
                                    ColorAnimation {
                                        duration: 240
                                        easing.type: Easing.OutCubic
                                    }
                                }
                            }
                        }
                    }

                    MetricSparkline {
                        id: memSpark
                        width: parent.width
                        samples: root.popoutSamples
                        valueKey: "mem_pct"
                        lineColor: memSection.memAccent
                        historyCapacity: root.historyCapacity
                    }
                }
            }
        }
    }
}
