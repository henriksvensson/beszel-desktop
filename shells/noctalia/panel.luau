--!nonstrict
-- Beszel Desktop live telemetry panel.
-- Configurable real-time view: host switcher, vitals strip, hover-scrubbable CPU,
-- memory, GPU, and dual-series network traces, per-core CPU heatmap, disk/pool health,
-- and optional top containers.

local fmt = require("./lib/format.luau")

local STATE_KEY = "beszel.data"
local SELECTED_HOST_KEY = "beszel.selected_host"
local OPENED_SOURCE_KEY = "beszel.opened_source"
local BAR_WIDGET_TYPE = "henriksvensson/beszel-desktop:bar"
local CORE_BAR_HEIGHT = 22
local NET_SCALE_FLOOR_BPS = 10 * 1024
local MAX_CONTAINERS_SHOWN = 5
local INSTALL_COMMAND =
  "curl -fsSL https://raw.githubusercontent.com/henriksvensson/beszel-desktop/main/packaging/install.sh | sh"
local INIT_COMMAND = "beszel-ctl init"

local data = noctalia.state.get(STATE_KEY)
local selectedHostId = noctalia.state.get(SELECTED_HOST_KEY)
local openedSource = noctalia.state.get(OPENED_SOURCE_KEY)
local isOpen = false
local copiedUntilMs = 0
local pendingCopyCommand = nil
local pendingPinWidgetId = nil
local pendingPinHostName = nil

local hoveredCpuIndex = nil
local hoveredMemIndex = nil
local hoveredGpuIndex = nil
local hoveredNetIndex = nil
local hoveredCoreIndex = nil
local animatedCores = {}
local targetCores = {}
local render

local function resetHoverState()
  hoveredCpuIndex = nil
  hoveredMemIndex = nil
  hoveredGpuIndex = nil
  hoveredNetIndex = nil
  hoveredCoreIndex = nil
end

local function activeSystem()
  local systems = type(data) == "table" and data.systems or {}
  return fmt.findSystemById(systems, selectedHostId) or systems[1]
end

local function listBeszelBarWidgets()
  local allWidgets = noctalia.getSetting("widget")
  if type(allWidgets) ~= "table" then
    return {}
  end
  local matched = {}
  for id, cfg in pairs(allWidgets) do
    if type(id) == "string" and type(cfg) == "table" and cfg.type == BAR_WIDGET_TYPE then
      table.insert(matched, {
        id = id,
        system = type(cfg.system) == "string" and noctalia.string.trim(cfg.system) or "",
      })
    end
  end
  table.sort(matched, function(a, b)
    return a.id < b.id
  end)
  return matched
end

local function resolveTargetBarWidget(widgets)
  if #widgets == 0 then
    return nil
  end
  if #widgets == 1 then
    return widgets[1]
  end
  local openedFilter = (type(openedSource) == "table" and type(openedSource.configuredFilter) == "string")
    and string.lower(noctalia.string.trim(openedSource.configuredFilter))
    or ""

  local exact = {}
  for _, w in ipairs(widgets) do
    if string.lower(w.system) == openedFilter then
      table.insert(exact, w)
    end
  end
  if #exact > 0 then
    -- When multiple widgets share the same unconfigured filter (e.g. a freshly added 2nd widget),
    -- target the newest one so the first widget stays untouched.
    return exact[#exact]
  end
  for i = #widgets, 1, -1 do
    if widgets[i].system == "" then
      return widgets[i]
    end
  end
  return widgets[1]
end

local function tomlQuoteString(value)
  local escaped = string.gsub(value, "\\", "\\\\")
  escaped = string.gsub(escaped, "\"", "\\\"")
  return "\"" .. escaped .. "\""
end

local function resolveNoctaliaBin()
  if noctalia.commandExists("noctalia") then
    return "noctalia"
  end
  local localBin = noctalia.expandPath("~/.local/bin/noctalia")
  if noctalia.fileExists(localBin) then
    return localBin
  end
  return "noctalia"
end

local function hostPeakColor(systemId, warnPct, dangerPct)
  local latest = fmt.latestSample(data, systemId)
  if latest == nil then
    return "on_surface_variant"
  end
  local peak = math.max(tonumber(latest.cpu_pct) or 0, tonumber(latest.mem_pct) or 0)
  return fmt.severityColor(peak, "primary", warnPct, dangerPct)
end

local function coreColor(pct, warnPct, dangerPct)
  if pct >= dangerPct then
    return "error"
  end
  if pct >= warnPct then
    return "tertiary"
  end
  if pct <= 4 then
    return "primary/0.22"
  end
  local ratio = fmt.clamp(pct / math.max(1, warnPct), 0, 1)
  local alpha = 0.42 + 0.58 * ratio
  return string.format("primary/%.2f", alpha)
end

local function coreColumnCount(count)
  if count <= 8 then
    return math.max(1, count)
  end
  if count <= 16 then
    return 8
  end
  if count <= 24 then
    return 12
  end
  return 16
end

local function extractSeries(samples, field)
  local values = {}
  if type(samples) ~= "table" then
    return values
  end
  for _, s in ipairs(samples) do
    local v = fmt.clamp((tonumber(s[field]) or 0) / 100.0, 0, 1)
    table.insert(values, v)
  end
  return values
end

local function extractGpuSeries(samples)
  local values = {}
  if type(samples) ~= "table" then
    return values
  end
  for _, s in ipairs(samples) do
    local gpu = fmt.primaryGpu(s)
    local pct = gpu ~= nil and (tonumber(gpu.usage_pct) or 0) or 0
    table.insert(values, fmt.clamp(pct / 100.0, 0, 1))
  end
  return values
end

local function extractNetworkSeries(samples)
  local recvNorm = {}
  local sentNorm = {}
  local hasAny = false
  local peakBps = 0

  if type(samples) == "table" then
    for _, s in ipairs(samples) do
      if fmt.hasNetwork(s) then
        hasAny = true
        local r = math.max(0, tonumber(s.net_recv_bps) or 0)
        local t = math.max(0, tonumber(s.net_sent_bps) or 0)
        if r > peakBps then
          peakBps = r
        end
        if t > peakBps then
          peakBps = t
        end
      end
    end
  end

  if not hasAny then
    return nil, nil, 0
  end

  local scaleMax = math.max(peakBps, NET_SCALE_FLOOR_BPS)
  for _, s in ipairs(samples) do
    local r = math.max(0, tonumber(s.net_recv_bps) or 0)
    local t = math.max(0, tonumber(s.net_sent_bps) or 0)
    table.insert(recvNorm, fmt.clamp(r / scaleMax, 0, 1))
    table.insert(sentNorm, fmt.clamp(t / scaleMax, 0, 1))
  end

  return recvNorm, sentNorm, peakBps
end

local function syncTargetCores(latest)
  targetCores = {}
  if latest ~= nil and type(latest.cpu_cores) == "table" then
    for i, val in ipairs(latest.cpu_cores) do
      targetCores[i] = fmt.clamp(tonumber(val) or 0, 0, 100)
    end
  end
  if #animatedCores ~= #targetCores then
    animatedCores = {}
    for i, val in ipairs(targetCores) do
      animatedCores[i] = val
    end
  end
end

local function buildHostChips(systems, currentId, warnPct, dangerPct)
  local rows = {}
  local currentRow = {}

  for _, sys in ipairs(systems) do
    local isSelected = currentId == sys.id
    table.insert(currentRow, ui.row({
      key = "host-" .. sys.id,
      gap = 6,
      paddingH = 10,
      paddingV = 5,
      align = "center",
      radius = 8,
      fill = isSelected and "primary_container" or "surface_container_high",
      onClick = function()
        selectedHostId = sys.id
        resetHoverState()
        animatedCores = {}
        noctalia.state.set(SELECTED_HOST_KEY, sys.id)
        render()
      end,
    }, {
      ui.box({
        width = 6,
        height = 6,
        radius = 3,
        fill = hostPeakColor(sys.id, warnPct, dangerPct),
      }),
      ui.label({
        text = sys.name,
        fontSize = 12,
        fontWeight = isSelected and "semibold" or "normal",
        color = isSelected and "on_primary_container" or "on_surface",
      }),
    }))

    if #currentRow >= 4 then
      table.insert(rows, ui.row({ gap = 6, align = "center" }, currentRow))
      currentRow = {}
    end
  end

  if #currentRow > 0 then
    table.insert(rows, ui.row({ gap = 6, align = "center" }, currentRow))
  end

  return ui.column({ gap = 6 }, rows)
end

local function buildVitalsStrip(latest)
  if noctalia.getConfig("show_vitals") == false then
    return nil
  end
  local pills = {}

  if latest.uptime_secs ~= nil then
    table.insert(pills, ui.label({
      text = noctalia.tr("panel.vitals_uptime", { uptime = fmt.formatUptime(latest.uptime_secs) }),
      fontSize = 11,
      color = "on_surface_variant",
      maxLines = 1,
    }))
  end

  if type(latest.load_avg) == "table" and #latest.load_avg >= 3 then
    table.insert(pills, ui.label({
      text = noctalia.tr("panel.vitals_load", {
        l1 = string.format("%.2f", tonumber(latest.load_avg[1]) or 0),
        l5 = string.format("%.2f", tonumber(latest.load_avg[2]) or 0),
        l15 = string.format("%.2f", tonumber(latest.load_avg[3]) or 0),
      }),
      fontSize = 11,
      color = "on_surface_variant",
      maxLines = 1,
    }))
  end

  if latest.temp_max_c ~= nil then
    local tColor = fmt.tempSeverityColor(latest.temp_max_c, "on_surface_variant")
    table.insert(pills, ui.row({ gap = 3, align = "center" }, {
      ui.glyph({ name = "temperature", size = 12, color = tColor }),
      ui.label({
        text = fmt.formatTemp(latest.temp_max_c),
        fontSize = 11,
        fontWeight = tColor ~= "on_surface_variant" and "semibold" or "normal",
        color = tColor,
        maxLines = 1,
      }),
    }))
  end

  if latest.battery_pct ~= nil then
    table.insert(pills, ui.row({ gap = 3, align = "center" }, {
      ui.glyph({ name = "battery", size = 12, color = "on_surface_variant" }),
      ui.label({
        text = string.format("%d%%", math.floor((tonumber(latest.battery_pct) or 0) + 0.5)),
        fontSize = 11,
        color = "on_surface_variant",
        maxLines = 1,
      }),
    }))
  end

  if #pills == 0 then
    return nil
  end

  return ui.row({
    gap = 10,
    paddingH = 10,
    paddingV = 6,
    radius = 8,
    fill = "surface_container_high",
    align = "center",
    justify = "space_between",
  }, pills)
end

local function buildCoreHeatmap(warnPct, dangerPct)
  if noctalia.getConfig("show_cores") == false then
    return nil
  end
  local count = #targetCores
  if count == 0 then
    return nil
  end

  local peak = 0
  for _, val in ipairs(targetCores) do
    if val > peak then
      peak = val
    end
  end

  local metaText
  if hoveredCoreIndex ~= nil and targetCores[hoveredCoreIndex] ~= nil then
    metaText = noctalia.tr("panel.cores_hover", {
      index = tostring(hoveredCoreIndex - 1),
      pct = tostring(math.floor(targetCores[hoveredCoreIndex] + 0.5)),
    })
  else
    metaText = noctalia.tr("panel.cores_summary", {
      count = tostring(count),
      peak = tostring(math.floor(peak + 0.5)),
    })
  end

  local cols = coreColumnCount(count)
  local gridRows = {}
  local idx = 1
  while idx <= count do
    local cells = {}
    for _ = 1, cols do
      local coreIdx = idx
      if coreIdx <= count then
        local shownVal = animatedCores[coreIdx] or targetCores[coreIdx] or 0
        local actualVal = math.floor((targetCores[coreIdx] or 0) + 0.5)
        local fillHeight = shownVal > 0.5
          and math.max(3, math.floor((shownVal / 100.0) * CORE_BAR_HEIGHT + 0.5))
          or 0
        local isHovered = hoveredCoreIndex == coreIdx

        local barChildren = {}
        if fillHeight > 0 then
          table.insert(barChildren, ui.box({
            height = fillHeight,
            radius = 2,
            fill = coreColor(shownVal, warnPct, dangerPct),
          }))
        end

        table.insert(cells, ui.column({
          key = "core-" .. tostring(coreIdx),
          flexGrow = 1,
          height = CORE_BAR_HEIGHT,
          justify = "end",
          radius = 3,
          fill = "surface_container_highest",
          border = isHovered and "on_surface/0.45" or nil,
          borderWidth = isHovered and 1 or nil,
          clip = true,
          onHover = "onCoreHover",
          tooltip = noctalia.tr("panel.cores_hover", {
            index = tostring(coreIdx - 1),
            pct = tostring(actualVal),
          }),
        }, barChildren))
      else
        table.insert(cells, ui.spacer({ flexGrow = 1 }))
      end
      idx += 1
    end
    table.insert(gridRows, ui.row({ gap = 4 }, cells))
  end

  return ui.column({ gap = 6 }, {
    ui.row({ align = "center", justify = "space_between" }, {
      ui.label({
        text = noctalia.tr("panel.cores"),
        fontSize = 13,
        fontWeight = "medium",
        color = "on_surface",
      }),
      ui.label({
        text = metaText,
        fontSize = 11,
        color = hoveredCoreIndex ~= nil and "on_surface" or "on_surface_variant",
      }),
    }),
    ui.column({
      gap = 4,
      padding = 8,
      radius = 8,
      fill = "surface_container_high",
    }, gridRows),
  })
end

local function buildGpuSection(samples, latest, warnPct, dangerPct)
  if noctalia.getConfig("show_gpu") == false then
    return nil
  end
  local latestGpu = fmt.primaryGpu(latest)
  if latestGpu == nil then
    return nil
  end

  local sample = (hoveredGpuIndex ~= nil and samples[hoveredGpuIndex]) or latest
  local gpu = fmt.primaryGpu(sample) or latestGpu
  local gpuPct = tonumber(gpu.usage_pct) or 0
  local gpuColor = fmt.severityColor(gpuPct, "primary", warnPct, dangerPct)

  local metaParts = {}
  if gpu.mem_used_mb ~= nil and gpu.mem_total_mb ~= nil then
    table.insert(
      metaParts,
      string.format("%s / %s", fmt.formatMb(gpu.mem_used_mb), fmt.formatMb(gpu.mem_total_mb))
    )
  end
  if gpu.power_w ~= nil then
    table.insert(metaParts, string.format("%.0f W", tonumber(gpu.power_w) or 0))
  end

  local rightChildren = {}
  if #metaParts > 0 then
    table.insert(rightChildren, ui.label({
      text = table.concat(metaParts, " · "),
      fontSize = 11,
      color = "on_surface_variant",
      maxLines = 1,
    }))
  end
  table.insert(rightChildren, ui.label({
    text = string.format("%.0f%%", gpuPct),
    fontSize = 16,
    fontWeight = "semibold",
    color = gpuColor,
  }))

  local title = (type(gpu.name) == "string" and gpu.name ~= "")
    and gpu.name
    or noctalia.tr("panel.gpu")

  return ui.column({ gap = 6 }, {
    ui.row({ align = "center", justify = "space_between", gap = 8 }, {
      ui.label({
        text = title,
        fontSize = 13,
        fontWeight = "medium",
        color = "on_surface",
        flexGrow = 1,
        maxLines = 1,
      }),
      ui.row({ gap = 8, align = "center" }, rightChildren),
    }),
    ui.column({
      padding = 6,
      radius = 8,
      fill = "surface_container_high",
    }, {
      ui.graph({
        key = "gpu-graph",
        height = 56,
        values = extractGpuSeries(samples),
        color = gpuColor,
        lineWidth = 2,
        fillOpacity = 0.22,
        onPointerMove = "onGpuGraphMove",
        onPointerLeave = "onGpuGraphLeave",
      }),
    }),
  })
end

local function buildNetworkSection(samples, latest)
  if noctalia.getConfig("show_network") == false then
    return nil
  end
  local recvSeries, sentSeries, peakBps = extractNetworkSeries(samples)
  if recvSeries == nil then
    return nil
  end

  local sample = (hoveredNetIndex ~= nil and samples[hoveredNetIndex]) or latest
  local recvBps = tonumber(sample.net_recv_bps) or 0
  local sentBps = tonumber(sample.net_sent_bps) or 0

  return ui.column({ gap = 6 }, {
    ui.row({ align = "center", justify = "space_between", gap = 8 }, {
      ui.label({
        text = noctalia.tr("panel.network"),
        fontSize = 13,
        fontWeight = "medium",
        color = "on_surface",
      }),
      ui.row({ gap = 10, align = "center" }, {
        ui.row({ gap = 3, align = "center" }, {
          ui.glyph({ name = "arrow-down", size = 12, color = "primary" }),
          ui.label({
            text = fmt.formatRate(recvBps),
            fontSize = 12,
            fontWeight = "semibold",
            color = "primary",
            maxLines = 1,
          }),
        }),
        ui.row({ gap = 3, align = "center" }, {
          ui.glyph({ name = "arrow-up", size = 12, color = "secondary" }),
          ui.label({
            text = fmt.formatRate(sentBps),
            fontSize = 12,
            fontWeight = "semibold",
            color = "secondary",
            maxLines = 1,
          }),
        }),
      }),
    }),
    ui.column({
      gap = 4,
      padding = 6,
      radius = 8,
      fill = "surface_container_high",
    }, {
      ui.graph({
        key = "net-graph",
        height = 56,
        values = recvSeries,
        values2 = sentSeries,
        color = "primary",
        color2 = "secondary",
        lineWidth = 2,
        fillOpacity = 0.18,
        onPointerMove = "onNetGraphMove",
        onPointerLeave = "onNetGraphLeave",
      }),
      ui.row({ align = "center", justify = "space_between" }, {
        ui.label({
          text = noctalia.tr("panel.network_legend"),
          fontSize = 10,
          color = "on_surface_variant",
        }),
        ui.label({
          text = noctalia.tr("panel.network_peak", { rate = fmt.formatRate(peakBps) }),
          fontSize = 10,
          color = "on_surface_variant",
        }),
      }),
    }),
  })
end

local function buildDiskSection(latest, warnPct, dangerPct)
  if noctalia.getConfig("show_disk") == false then
    return nil
  end
  local hasDiskPct = latest.disk_pct ~= nil
  local hasDiskIo = latest.disk_read_bps ~= nil or latest.disk_write_bps ~= nil
  local unhealthy = type(latest.unhealthy_pools) == "table" and latest.unhealthy_pools or {}
  if not hasDiskPct and not hasDiskIo and #unhealthy == 0 then
    return nil
  end

  local headerRight = {}
  local cardChildren = {}

  if hasDiskPct then
    local pct = tonumber(latest.disk_pct) or 0
    local diskColor = fmt.severityColor(pct, "primary", warnPct, dangerPct)
    if latest.disk_used_gb ~= nil and latest.disk_total_gb ~= nil then
      table.insert(headerRight, ui.label({
        text = string.format(
          "%.0f / %.0f GiB",
          tonumber(latest.disk_used_gb) or 0,
          tonumber(latest.disk_total_gb) or 0
        ),
        fontSize = 11,
        color = "on_surface_variant",
      }))
    end
    table.insert(headerRight, ui.label({
      text = string.format("%.0f%%", pct),
      fontSize = 15,
      fontWeight = "semibold",
      color = diskColor,
    }))
    table.insert(cardChildren, ui.progress({
      progress = fmt.clamp(pct / 100.0, 0, 1),
      fill = diskColor,
      track = "surface_container_highest",
      height = 6,
      radius = 3,
    }))
  end

  if hasDiskIo then
    table.insert(cardChildren, ui.row({ align = "center", justify = "space_between" }, {
      ui.label({
        text = noctalia.tr("panel.disk_io"),
        fontSize = 11,
        color = "on_surface_variant",
      }),
      ui.label({
        text = noctalia.tr("panel.disk_io_values", {
          read = fmt.formatRate(latest.disk_read_bps or 0),
          write = fmt.formatRate(latest.disk_write_bps or 0),
        }),
        fontSize = 11,
        color = "on_surface",
      }),
    }))
  end

  if #unhealthy > 0 then
    table.insert(cardChildren, ui.row({
      gap = 6,
      paddingH = 8,
      paddingV = 5,
      radius = 6,
      fill = "error_container",
      align = "center",
    }, {
      ui.glyph({ name = "alert-triangle", size = 13, color = "on_error_container" }),
      ui.label({
        text = noctalia.tr("panel.pool_degraded", { pools = table.concat(unhealthy, ", ") }),
        fontSize = 11,
        fontWeight = "medium",
        color = "on_error_container",
        maxLines = 2,
      }),
    }))
  end

  return ui.column({ gap = 6 }, {
    ui.row({ align = "center", justify = "space_between" }, {
      ui.label({
        text = noctalia.tr("panel.disk"),
        fontSize = 13,
        fontWeight = "medium",
        color = "on_surface",
      }),
      ui.row({ gap = 8, align = "center" }, headerRight),
    }),
    ui.column({
      gap = 8,
      padding = 10,
      radius = 8,
      fill = "surface_container_high",
    }, cardChildren),
  })
end

local function buildContainersSection(latest)
  if noctalia.getConfig("show_containers") ~= true then
    return nil
  end
  if type(latest.containers) ~= "table" or #latest.containers == 0 then
    return nil
  end

  local rows = {}
  local limit = math.min(#latest.containers, MAX_CONTAINERS_SHOWN)
  for i = 1, limit do
    local c = latest.containers[i]
    table.insert(rows, ui.row({
      key = "ctr-" .. tostring(i) .. "-" .. tostring(c.name or ""),
      align = "center",
      justify = "space_between",
      gap = 8,
    }, {
      ui.label({
        text = tostring(c.name or ""),
        fontSize = 11,
        color = "on_surface",
        flexGrow = 1,
        maxLines = 1,
      }),
      ui.row({ gap = 10, align = "center" }, {
        ui.label({
          text = string.format("%.1f%%", tonumber(c.cpu_pct) or 0),
          width = 40,
          textAlign = "end",
          fontSize = 11,
          fontWeight = "medium",
          color = "on_surface",
        }),
        ui.label({
          text = fmt.formatMb(c.mem_mb or 0),
          width = 58,
          textAlign = "end",
          fontSize = 11,
          color = "on_surface_variant",
        }),
      }),
    }))
  end

  return ui.column({ gap = 6 }, {
    ui.row({ align = "center", justify = "space_between" }, {
      ui.label({
        text = noctalia.tr("panel.containers"),
        fontSize = 13,
        fontWeight = "medium",
        color = "on_surface",
      }),
      ui.label({
        text = noctalia.tr("panel.containers_count", { count = tostring(#latest.containers) }),
        fontSize = 11,
        color = "on_surface_variant",
      }),
    }),
    ui.column({
      gap = 6,
      padding = 10,
      radius = 8,
      fill = "surface_container_high",
    }, rows),
  })
end

local function buildOnboardingCard(state)
  local isMissing = type(data) == "table" and data.ctlMissing == true
  local isCopied = noctalia.nowMs() < copiedUntilMs

  if isMissing or state == "disconnected" or state == "error" then
    local headingKey = isMissing and "panel.setup_install_title"
      or (state == "error" and "panel.setup_auth_title" or "panel.setup_init_title")
    local descKey = isMissing and "panel.setup_install_desc"
      or (state == "error" and "panel.setup_auth_desc" or "panel.setup_init_desc")
    local cmd = isMissing and INSTALL_COMMAND or INIT_COMMAND
    local btnKey = isCopied and "panel.setup_copied"
      or (isMissing and "panel.setup_copy_install" or "panel.setup_copy_init")

    pendingCopyCommand = cmd

    local children = {
      ui.label({
        text = noctalia.tr(headingKey),
        fontSize = 13,
        fontWeight = "semibold",
        color = "on_surface",
      }),
      ui.label({
        text = noctalia.tr(descKey),
        fontSize = 12,
        color = "on_surface_variant",
        maxLines = 4,
      }),
      ui.column({
        paddingH = 10,
        paddingV = 8,
        radius = 6,
        fill = "surface_container_highest",
      }, {
        ui.label({
          text = cmd,
          fontSize = 11,
          fontFamily = "monospace",
          color = "on_surface",
          maxLines = 3,
        }),
      }),
      ui.button({
        text = noctalia.tr(btnKey),
        glyph = isCopied and "check" or "copy",
        glyphSize = 14,
        variant = "primary",
        controlSize = "sm",
        onClick = "onCopySetupCommand",
      }),
    }

    if isMissing then
      table.insert(children, ui.label({
        text = noctalia.tr("panel.setup_alt"),
        fontSize = 11,
        color = "on_surface_variant",
        maxLines = 2,
      }))
    end

    return ui.column({
      gap = 10,
      padding = 12,
      radius = 8,
      fill = "surface_container_high",
    }, children)
  end

  pendingCopyCommand = nil
  return ui.column({
    padding = 16,
    radius = 8,
    fill = "surface_container_high",
    align = "center",
  }, {
    ui.label({
      text = noctalia.tr("panel.empty"),
      fontSize = 12,
      color = "on_surface_variant",
      maxLines = 2,
    }),
  })
end

render = function()
  if not isOpen then
    return
  end

  local warnPct, dangerPct = fmt.thresholds()
  local systems = type(data) == "table" and data.systems or {}
  local sys = activeSystem()
  local samples = (sys ~= nil and type(data) == "table" and type(data.historyBySystem) == "table")
    and (data.historyBySystem[sys.id] or {})
    or {}
  local latest = #samples > 0 and samples[#samples] or nil
  local state = fmt.effectiveState(data, sys, latest, "")

  syncTargetCores(latest)

  local title = sys ~= nil and sys.name or noctalia.tr("panel.title")
  local statusText = ""
  if type(data) == "table" and type(data.connection) == "table" and data.connection.last_error ~= nil then
    statusText = fmt.stateLabel(state, "") .. ": " .. tostring(data.connection.last_error)
  elseif state ~= "connected" then
    statusText = fmt.stateLabel(state, "")
  end

  local body = {}

  -- Resolve whether the viewed host can be pinned to the bar widget that opened this panel
  local barWidgets = listBeszelBarWidgets()
  local targetWidget = resolveTargetBarWidget(barWidgets)
  local showPinButton = false
  pendingPinWidgetId = nil
  pendingPinHostName = nil

  if sys ~= nil and targetWidget ~= nil and #systems > 1 then
    local widgetResolved = fmt.findSystem(systems, targetWidget.system)
    local alreadyExplicitlyPinned = targetWidget.system ~= ""
      and widgetResolved ~= nil
      and widgetResolved.id == sys.id
    local singleDefaultMatch = #barWidgets == 1
      and targetWidget.system == ""
      and widgetResolved ~= nil
      and widgetResolved.id == sys.id
    if not alreadyExplicitlyPinned and not singleDefaultMatch then
      showPinButton = true
      pendingPinWidgetId = targetWidget.id
      pendingPinHostName = sys.name
    end
  end

  local headerActions = {}
  if showPinButton then
    table.insert(headerActions, ui.button({
      glyph = "pin",
      glyphSize = 14,
      variant = "ghost",
      controlSize = "sm",
      tooltip = noctalia.tr("panel.pin_to_bar_tooltip", { host = sys.name }),
      onClick = "onPinToBar",
    }))
  end
  table.insert(headerActions, ui.button({
    glyph = "settings",
    glyphSize = 14,
    variant = "ghost",
    controlSize = "sm",
    tooltip = noctalia.tr("panel.open_settings"),
    onClick = "onOpenSettings",
  }))

  -- Header row (pinned above scroll)
  table.insert(body, ui.row({ align = "center", justify = "space_between", gap = 8 }, {
    ui.column({ gap = 2, flexGrow = 1 }, {
      ui.label({
        text = title,
        fontSize = 16,
        fontWeight = "semibold",
        color = "on_surface",
      }),
      ui.label({
        text = statusText,
        visible = statusText ~= "",
        fontSize = 11,
        color = fmt.stateColor(state),
        maxLines = 2,
      }),
    }),
    ui.row({ gap = 4, align = "center" }, headerActions),
  }))

  -- Host switcher chips (pinned above scroll)
  if #systems > 1 then
    table.insert(body, buildHostChips(systems, sys ~= nil and sys.id or nil, warnPct, dangerPct))
  end

  if latest == nil then
    table.insert(body, buildOnboardingCard(state))
    panel.render(ui.column({ gap = 12, padding = 12 }, body))
    return
  end

  local sections = {}

  local vitals = buildVitalsStrip(latest)
  if vitals ~= nil then
    table.insert(sections, vitals)
  end

  -- CPU section
  local cpuSample = (hoveredCpuIndex ~= nil and samples[hoveredCpuIndex]) or latest
  local cpuPct = tonumber(cpuSample.cpu_pct) or 0
  local cpuColor = fmt.severityColor(cpuPct, "primary", warnPct, dangerPct)

  table.insert(sections, ui.column({ gap = 6 }, {
    ui.row({ align = "center", justify = "space_between" }, {
      ui.label({
        text = noctalia.tr("panel.cpu"),
        fontSize = 13,
        fontWeight = "medium",
        color = "on_surface",
      }),
      ui.label({
        text = string.format("%.1f%%", cpuPct),
        fontSize = 16,
        fontWeight = "semibold",
        color = cpuColor,
      }),
    }),
    ui.column({
      padding = 6,
      radius = 8,
      fill = "surface_container_high",
    }, {
      ui.graph({
        key = "cpu-graph",
        height = 64,
        values = extractSeries(samples, "cpu_pct"),
        color = cpuColor,
        lineWidth = 2,
        fillOpacity = 0.22,
        onPointerMove = "onCpuGraphMove",
        onPointerLeave = "onCpuGraphLeave",
      }),
    }),
  }))

  -- Per-core CPU heatmap
  local heatmap = buildCoreHeatmap(warnPct, dangerPct)
  if heatmap ~= nil then
    table.insert(sections, heatmap)
  end

  -- Memory section (+ optional swap readout when used > 0)
  local memSample = (hoveredMemIndex ~= nil and samples[hoveredMemIndex]) or latest
  local memPct = tonumber(memSample.mem_pct) or 0
  local memColor = fmt.severityColor(memPct, "secondary", warnPct, dangerPct)

  local memReadoutChildren = {}
  if memSample.mem_used_bytes ~= nil and memSample.mem_total_bytes ~= nil then
    table.insert(memReadoutChildren, ui.label({
      text = string.format(
        "%s / %s GiB",
        fmt.formatGiB(memSample.mem_used_bytes),
        fmt.formatGiB(memSample.mem_total_bytes)
      ),
      fontSize = 11,
      color = "on_surface_variant",
    }))
  end
  table.insert(memReadoutChildren, ui.label({
    text = string.format("%.1f%%", memPct),
    fontSize = 16,
    fontWeight = "semibold",
    color = memColor,
  }))

  local memCardChildren = {
    ui.graph({
      key = "mem-graph",
      height = 64,
      values = extractSeries(samples, "mem_pct"),
      color = memColor,
      lineWidth = 2,
      fillOpacity = 0.22,
      onPointerMove = "onMemGraphMove",
      onPointerLeave = "onMemGraphLeave",
    }),
  }

  local swapUsed = tonumber(memSample.swap_used_bytes) or 0
  if noctalia.getConfig("show_swap") ~= false and swapUsed > 0 and memSample.swap_total_bytes ~= nil then
    table.insert(memCardChildren, ui.row({ align = "center", justify = "space_between" }, {
      ui.label({
        text = noctalia.tr("panel.swap"),
        fontSize = 10,
        color = "on_surface_variant",
      }),
      ui.label({
        text = string.format(
          "%s / %s GiB",
          fmt.formatGiB(swapUsed),
          fmt.formatGiB(memSample.swap_total_bytes)
        ),
        fontSize = 10,
        color = "on_surface_variant",
      }),
    }))
  end

  table.insert(sections, ui.column({ gap = 6 }, {
    ui.row({ align = "center", justify = "space_between" }, {
      ui.label({
        text = noctalia.tr("panel.memory"),
        fontSize = 13,
        fontWeight = "medium",
        color = "on_surface",
      }),
      ui.row({ gap = 8, align = "center" }, memReadoutChildren),
    }),
    ui.column({
      gap = 4,
      padding = 6,
      radius = 8,
      fill = "surface_container_high",
    }, memCardChildren),
  }))

  local gpuSection = buildGpuSection(samples, latest, warnPct, dangerPct)
  if gpuSection ~= nil then
    table.insert(sections, gpuSection)
  end

  local netSection = buildNetworkSection(samples, latest)
  if netSection ~= nil then
    table.insert(sections, netSection)
  end

  local diskSection = buildDiskSection(latest, warnPct, dangerPct)
  if diskSection ~= nil then
    table.insert(sections, diskSection)
  end

  local containersSection = buildContainersSection(latest)
  if containersSection ~= nil then
    table.insert(sections, containersSection)
  end

  table.insert(body, ui.scroll({ key = "metrics-scroll", gap = 12, flexGrow = 1 }, sections))

  panel.render(ui.column({ gap = 12, padding = 12 }, body))
end

local function sampleIndexFromNormX(normX)
  local sys = activeSystem()
  if sys == nil or type(data) ~= "table" or type(data.historyBySystem) ~= "table" then
    return nil
  end
  local samples = data.historyBySystem[sys.id]
  if type(samples) ~= "table" or #samples == 0 then
    return nil
  end
  local x = fmt.clamp(tonumber(normX) or 0, 0, 1)
  local idx = math.floor(x * (#samples - 1) + 0.5) + 1
  return fmt.clamp(idx, 1, #samples)
end

function onCpuGraphMove(normX)
  local nextIdx = sampleIndexFromNormX(normX)
  if nextIdx ~= hoveredCpuIndex then
    hoveredCpuIndex = nextIdx
    render()
  end
end

function onCpuGraphLeave()
  if hoveredCpuIndex ~= nil then
    hoveredCpuIndex = nil
    render()
  end
end

function onMemGraphMove(normX)
  local nextIdx = sampleIndexFromNormX(normX)
  if nextIdx ~= hoveredMemIndex then
    hoveredMemIndex = nextIdx
    render()
  end
end

function onMemGraphLeave()
  if hoveredMemIndex ~= nil then
    hoveredMemIndex = nil
    render()
  end
end

function onGpuGraphMove(normX)
  local nextIdx = sampleIndexFromNormX(normX)
  if nextIdx ~= hoveredGpuIndex then
    hoveredGpuIndex = nextIdx
    render()
  end
end

function onGpuGraphLeave()
  if hoveredGpuIndex ~= nil then
    hoveredGpuIndex = nil
    render()
  end
end

function onNetGraphMove(normX)
  local nextIdx = sampleIndexFromNormX(normX)
  if nextIdx ~= hoveredNetIndex then
    hoveredNetIndex = nextIdx
    render()
  end
end

function onNetGraphLeave()
  if hoveredNetIndex ~= nil then
    hoveredNetIndex = nil
    render()
  end
end

function onCoreHover(hoverState, key)
  local nextCore = nil
  if hoverState == "true" and type(key) == "string" then
    nextCore = tonumber(string.match(key, "^core%-(%d+)$"))
  end
  if nextCore ~= hoveredCoreIndex then
    hoveredCoreIndex = nextCore
    render()
  end
end

function onOpenSettings()
  noctalia.openSettings()
end

function onPinToBar()
  if type(pendingPinWidgetId) ~= "string" or type(pendingPinHostName) ~= "string" then
    return
  end
  local widgetId = pendingPinWidgetId
  local hostName = pendingPinHostName
  noctalia.runAsync({
    resolveNoctaliaBin(),
    "msg",
    "config-set",
    "widget." .. widgetId .. ".system",
    tomlQuoteString(hostName),
  }, function(res)
    if res ~= nil and res.exitCode == 0 and not res.timedOut then
      openedSource = {
        configuredFilter = hostName,
        displayedHostId = selectedHostId,
      }
      noctalia.state.set(OPENED_SOURCE_KEY, openedSource)
      if isOpen then
        render()
      end
    else
      noctalia.notifyError(
        noctalia.tr("panel.title"),
        noctalia.tr("panel.pin_failed_toast")
      )
    end
  end)
end

function onCopySetupCommand()
  if type(pendingCopyCommand) ~= "string" or pendingCopyCommand == "" then
    return
  end
  if noctalia.copyToClipboard(pendingCopyCommand, "text/plain") then
    copiedUntilMs = noctalia.nowMs() + 2500
    render()
  end
end

function onFrameTick(deltaMs)
  if not isOpen or #targetCores == 0 then
    return
  end
  local dt = fmt.clamp((tonumber(deltaMs) or 16) / 1000.0, 0.001, 0.1)
  local alpha = 1.0 - math.exp(-dt * 12.0)
  local changed = false

  for i, target in ipairs(targetCores) do
    local cur = animatedCores[i] or target
    local diff = target - cur
    if math.abs(diff) > 0.2 then
      animatedCores[i] = cur + diff * alpha
      changed = true
    elseif cur ~= target then
      animatedCores[i] = target
      changed = true
    end
  end

  if changed then
    render()
  end
end

noctalia.state.watch(STATE_KEY, function(nextData)
  data = nextData
  if isOpen then
    render()
  end
end)

noctalia.state.watch(SELECTED_HOST_KEY, function(hostId)
  if type(hostId) == "string" and hostId ~= "" and hostId ~= selectedHostId then
    selectedHostId = hostId
    resetHoverState()
    animatedCores = {}
    if isOpen then
      render()
    end
  end
end)

noctalia.state.watch(OPENED_SOURCE_KEY, function(nextSource)
  openedSource = nextSource
  if isOpen then
    render()
  end
end)

function onOpen(context)
  isOpen = true
  if type(context) == "string" and context ~= "" then
    local systems = type(data) == "table" and data.systems or {}
    local matched = fmt.findSystem(systems, context)
    if matched ~= nil then
      selectedHostId = matched.id
    end
  end
  resetHoverState()
  panel.setWantsSecondTicks(true)
  panel.setNeedsFrameTick(true)
  render()
end

function onClose()
  isOpen = false
  resetHoverState()
  panel.setWantsSecondTicks(false)
  panel.setNeedsFrameTick(false)
end

function update()
  if isOpen then
    render()
  end
end

function onConfigChanged()
  if isOpen then
    render()
  end
end
