--!nonstrict
-- Beszel Desktop bar widget.
-- Supports multiple independent host instances via per-widget `system` setting.

local fmt = require("./lib/format.luau")

local PANEL_ID = "henriksvensson/beszel-desktop:panel"
local STATE_KEY = "beszel.data"
local SELECTED_HOST_KEY = "beszel.selected_host"

local data = noctalia.state.get(STATE_KEY)

noctalia.setUpdateInterval(1000)

local function buildTooltip(sys, latest, state, filter)
  local rows = {}
  if sys ~= nil then
    table.insert(rows, { key = noctalia.tr("tooltip.host"), value = sys.name })
  end
  if latest ~= nil then
    table.insert(rows, {
      key = noctalia.tr("tooltip.cpu"),
      value = string.format("%.1f%%", tonumber(latest.cpu_pct) or 0),
    })
    local memText = string.format("%.1f%%", tonumber(latest.mem_pct) or 0)
    if latest.mem_used_bytes ~= nil and latest.mem_total_bytes ~= nil then
      memText = string.format(
        "%s (%s / %s GiB)",
        memText,
        fmt.formatGiB(latest.mem_used_bytes),
        fmt.formatGiB(latest.mem_total_bytes)
      )
    end
    table.insert(rows, { key = noctalia.tr("tooltip.memory"), value = memText })
  end
  table.insert(rows, { key = noctalia.tr("tooltip.status"), value = fmt.stateLabel(state, filter) })
  return rows
end

local function render()
  local filter = noctalia.getConfig("system") or ""
  local showHost = noctalia.getConfig("show_host_name") ~= false
  local warnPct, dangerPct = fmt.thresholds()

  local systems = type(data) == "table" and data.systems or {}
  local sys = fmt.findSystem(systems, filter)
  local latest = sys ~= nil and fmt.latestSample(data, sys.id) or nil
  local state = fmt.effectiveState(data, sys, latest, filter)
  local statusColor = fmt.stateColor(state)

  barWidget.setTooltip(buildTooltip(sys, latest, state, filter))

  local showMetrics = latest ~= nil
    and state ~= "disconnected"
    and state ~= "auth_error"
    and state ~= "not_found"

  if not showMetrics then
    barWidget.render(ui.label({
      text = fmt.stateLabel(state, filter),
      color = statusColor,
      maxLines = 1,
    }))
    return
  end

  local cpuPct = tonumber(latest.cpu_pct) or 0
  local memPct = tonumber(latest.mem_pct) or 0

  local cpuIconColor = state == "connected"
    and fmt.severityColor(cpuPct, "primary", warnPct, dangerPct)
    or statusColor
  local cpuTextColor = state == "connected"
    and fmt.severityColor(cpuPct, "on_surface", warnPct, dangerPct)
    or statusColor
  local memIconColor = state == "connected"
    and fmt.severityColor(memPct, "secondary", warnPct, dangerPct)
    or statusColor
  local memTextColor = state == "connected"
    and fmt.severityColor(memPct, "on_surface", warnPct, dangerPct)
    or statusColor

  if barWidget.isVertical() then
    barWidget.render(ui.column({ gap = 2, align = "center" }, {
      ui.label({
        text = fmt.padPercent(cpuPct),
        color = cpuTextColor,
        fontWeight = "medium",
        maxLines = 1,
      }),
      ui.label({
        text = fmt.padPercent(memPct),
        color = state == "connected" and fmt.severityColor(memPct, "on_surface_variant", warnPct, dangerPct) or statusColor,
        maxLines = 1,
      }),
    }))
    return
  end

  local items = {}
  if showHost and sys ~= nil then
    table.insert(items, ui.label({
      text = sys.name,
      color = state == "connected" and "on_surface" or statusColor,
      maxLines = 1,
    }))
  end

  table.insert(items, ui.row({ gap = 3, align = "center" }, {
    ui.glyph({ name = "cpu-usage", size = 14, color = cpuIconColor }),
    ui.label({
      text = fmt.padPercent(cpuPct),
      width = 30,
      textAlign = "end",
      color = cpuTextColor,
      maxLines = 1,
    }),
  }))

  table.insert(items, ui.row({ gap = 3, align = "center" }, {
    ui.glyph({ name = "memory", size = 14, color = memIconColor }),
    ui.label({
      text = fmt.padPercent(memPct),
      width = 30,
      textAlign = "end",
      color = memTextColor,
      maxLines = 1,
    }),
  }))

  barWidget.render(ui.row({ gap = 8, align = "center" }, items))
end

noctalia.state.watch(STATE_KEY, function(nextData)
  data = nextData
  render()
end)

function update()
  render()
end

function onConfigChanged()
  render()
end

function onClick()
  local filter = noctalia.getConfig("system") or ""
  local systems = type(data) == "table" and data.systems or {}
  local sys = fmt.findSystem(systems, filter)
  if sys ~= nil then
    noctalia.state.set(SELECTED_HOST_KEY, sys.id)
  end
  noctalia.togglePanel(PANEL_ID)
end

render()
