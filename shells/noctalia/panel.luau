--!nonstrict
-- Beszel Desktop live telemetry panel.
-- Focused per-host dashboard: vitals strip, 2-column balanced grid of hover-scrubbable
-- CPU, memory, GPU, and dual-series network traces, per-core CPU heatmap, disk/pool
-- health, and optional top containers, inside a scrollable viewport.

local fmt = require("./lib/format.luau")

local STATE_KEY = "beszel.data"
local SELECTED_HOST_KEY = "beszel.selected_host"
local CORE_BAR_HEIGHT = 22
local NET_SCALE_FLOOR_BPS = 10 * 1024
local MAX_CONTAINERS_SHOWN = 5
local INSTALL_COMMAND =
  "curl -fsSL https://raw.githubusercontent.com/henriksvensson/beszel-desktop/main/packaging/install.sh | sh"
local INIT_COMMAND = "beszel-ctl init"

local data = noctalia.state.get(STATE_KEY)
local selectedHostId = noctalia.state.get(SELECTED_HOST_KEY)
local isOpen = false
local copiedUntilMs = 0
local pendingCopyCommand = nil

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

  if noctalia.getConfig("show_load") == true and type(latest.load_avg) == "table" and #latest.load_avg >= 3 then
    local coreCount = type(latest.cpu_cores) == "table" and #latest.cpu_cores or 0
    local loadText
    if coreCount > 0 then
      loadText = noctalia.tr("panel.vitals_load_cores", {
        l1 = string.format("%.2f", tonumber(latest.load_avg[1]) or 0),
        l5 = string.format("%.2f", tonumber(latest.load_avg[2]) or 0),
        l15 = string.format("%.2f", tonumber(latest.load_avg[3]) or 0),
        cores = tostring(coreCount),
      })
    else
      loadText = noctalia.tr("panel.vitals_load", {
        l1 = string.format("%.2f", tonumber(latest.load_avg[1]) or 0),
        l5 = string.format("%.2f", tonumber(latest.load_avg[2]) or 0),
        l15 = string.format("%.2f", tonumber(latest.load_avg[3]) or 0),
      })
    end
    table.insert(pills, ui.label({
      text = loadText,
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
    gap = 14,
    paddingH = 10,
    paddingV = 6,
    radius = 8,
    fill = "surface_container_high",
    align = "center",
    justify = #pills > 1 and "space_between" or "start",
  }, pills)
end

local function buildCpuSection(samples, latest, warnPct, dangerPct)
  local cpuSample = (hoveredCpuIndex ~= nil and samples[hoveredCpuIndex]) or latest
  local cpuPct = tonumber(cpuSample.cpu_pct) or 0
  local cpuColor = fmt.severityColor(cpuPct, "primary", warnPct, dangerPct)

  return ui.column({ gap = 6 }, {
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
        height = 68,
        values = extractSeries(samples, "cpu_pct"),
        color = cpuColor,
        lineWidth = 2,
        fillOpacity = 0.22,
        onPointerMove = "onCpuGraphMove",
        onPointerLeave = "onCpuGraphLeave",
      }),
    }),
  })
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

  local estHeight = 46 + (#gridRows * (CORE_BAR_HEIGHT + 4))
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
  }), estHeight
end

local function buildMemorySection(samples, latest, warnPct, dangerPct)
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
      height = 68,
      values = extractSeries(samples, "mem_pct"),
      color = memColor,
      lineWidth = 2,
      fillOpacity = 0.22,
      onPointerMove = "onMemGraphMove",
      onPointerLeave = "onMemGraphLeave",
    }),
  }

  local estHeight = 104
  local swapUsed = tonumber(memSample.swap_used_bytes) or 0
  if noctalia.getConfig("show_swap") ~= false and swapUsed > 0 and memSample.swap_total_bytes ~= nil then
    estHeight += 18
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

  return ui.column({ gap = 6 }, {
    ui.row({ align = "center", justify = "space_between", gap = 6 }, {
      ui.label({
        text = noctalia.tr("panel.memory"),
        fontSize = 13,
        fontWeight = "medium",
        color = "on_surface",
      }),
      ui.row({ gap = 6, align = "center" }, memReadoutChildren),
    }),
    ui.column({
      gap = 4,
      padding = 6,
      radius = 8,
      fill = "surface_container_high",
    }, memCardChildren),
  }), estHeight
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
      ui.row({ gap = 6, align = "center" }, rightChildren),
    }),
    ui.column({
      padding = 6,
      radius = 8,
      fill = "surface_container_high",
    }, {
      ui.graph({
        key = "gpu-graph",
        height = 60,
        values = extractGpuSeries(samples),
        color = gpuColor,
        lineWidth = 2,
        fillOpacity = 0.22,
        onPointerMove = "onGpuGraphMove",
        onPointerLeave = "onGpuGraphLeave",
      }),
    }),
  }), 96
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

  -- Use distinct high-contrast palette roles: primary (download) vs tertiary (upload).
  local dlColor = "primary"
  local ulColor = "tertiary"

  return ui.column({ gap = 6 }, {
    ui.row({ align = "center", justify = "space_between", gap = 6 }, {
      ui.label({
        text = noctalia.tr("panel.network"),
        fontSize = 13,
        fontWeight = "medium",
        color = "on_surface",
      }),
      ui.row({ gap = 8, align = "center" }, {
        ui.row({ gap = 2, align = "center" }, {
          ui.glyph({ name = "arrow-down", size = 12, color = dlColor }),
          ui.label({
            text = fmt.formatRate(recvBps),
            fontSize = 12,
            fontWeight = "semibold",
            color = dlColor,
            maxLines = 1,
          }),
        }),
        ui.row({ gap = 2, align = "center" }, {
          ui.glyph({ name = "arrow-up", size = 12, color = ulColor }),
          ui.label({
            text = fmt.formatRate(sentBps),
            fontSize = 12,
            fontWeight = "semibold",
            color = ulColor,
            maxLines = 1,
          }),
        }),
      }),
    }),
    ui.column({
      gap = 5,
      padding = 6,
      radius = 8,
      fill = "surface_container_high",
    }, {
      ui.graph({
        key = "net-graph",
        height = 60,
        values = recvSeries,
        values2 = sentSeries,
        color = dlColor,
        color2 = ulColor,
        lineWidth = 2,
        fillOpacity = 0.18,
        onPointerMove = "onNetGraphMove",
        onPointerLeave = "onNetGraphLeave",
      }),
      ui.row({ align = "center", justify = "space_between" }, {
        ui.row({ gap = 10, align = "center" }, {
          ui.row({ gap = 4, align = "center" }, {
            ui.box({ width = 7, height = 7, radius = 4, fill = dlColor }),
            ui.label({
              text = noctalia.tr("panel.network_download"),
              fontSize = 10,
              color = "on_surface_variant",
            }),
          }),
          ui.row({ gap = 4, align = "center" }, {
            ui.box({ width = 7, height = 7, radius = 4, fill = ulColor }),
            ui.label({
              text = noctalia.tr("panel.network_upload"),
              fontSize = 10,
              color = "on_surface_variant",
            }),
          }),
        }),
        ui.label({
          text = noctalia.tr("panel.network_peak", { rate = fmt.formatRate(peakBps) }),
          fontSize = 10,
          color = "on_surface_variant",
        }),
      }),
    }),
  }), 112
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
  local estHeight = 56

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
    estHeight += 14
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
    estHeight += 22
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
    estHeight += 36
  end

  return ui.column({ gap = 6 }, {
    ui.row({ align = "center", justify = "space_between" }, {
      ui.label({
        text = noctalia.tr("panel.disk"),
        fontSize = 13,
        fontWeight = "medium",
        color = "on_surface",
      }),
      ui.row({ gap = 6, align = "center" }, headerRight),
    }),
    ui.column({
      gap = 8,
      padding = 10,
      radius = 8,
      fill = "surface_container_high",
    }, cardChildren),
  }), estHeight
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
      ui.row({ gap = 8, align = "center" }, {
        ui.label({
          text = string.format("%.1f%%", tonumber(c.cpu_pct) or 0),
          width = 38,
          textAlign = "end",
          fontSize = 11,
          fontWeight = "medium",
          color = "on_surface",
        }),
        ui.label({
          text = fmt.formatMb(c.mem_mb or 0),
          width = 56,
          textAlign = "end",
          fontSize = 11,
          color = "on_surface_variant",
        }),
      }),
    }))
  end

  local estHeight = 52 + (limit * 20)
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
  }), estHeight
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

  local header = ui.row({ align = "center", justify = "space_between", gap = 8 }, {
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
    ui.button({
      glyph = "settings",
      glyphSize = 14,
      variant = "ghost",
      controlSize = "sm",
      tooltip = noctalia.tr("panel.open_settings"),
      onClick = "onOpenSettings",
    }),
  })

  if latest == nil then
    panel.render(ui.column({ flexGrow = 1, gap = 12 }, {
      header,
      buildOnboardingCard(state),
    }))
    return
  end

  local scrollChildren = {}

  local vitals = buildVitalsStrip(latest)
  if vitals ~= nil then
    table.insert(scrollChildren, vitals)
  end

  -- Two balanced columns: CPU pinned top-left, Memory pinned top-right, remaining
  -- sections placed into whichever column is currently shorter.
  local leftCol = { buildCpuSection(samples, latest, warnPct, dangerPct) }
  local leftHeight = 104

  local memNode, memHeight = buildMemorySection(samples, latest, warnPct, dangerPct)
  local rightCol = { memNode }
  local rightHeight = memHeight

  local extras = {}
  local function addExtra(node, height)
    if node ~= nil then
      table.insert(extras, { node = node, height = height or 96 })
    end
  end

  local heatmapNode, heatmapHeight = buildCoreHeatmap(warnPct, dangerPct)
  addExtra(heatmapNode, heatmapHeight)
  local netNode, netHeight = buildNetworkSection(samples, latest)
  addExtra(netNode, netHeight)
  local gpuNode, gpuHeight = buildGpuSection(samples, latest, warnPct, dangerPct)
  addExtra(gpuNode, gpuHeight)
  local diskNode, diskHeight = buildDiskSection(latest, warnPct, dangerPct)
  addExtra(diskNode, diskHeight)
  local ctrNode, ctrHeight = buildContainersSection(latest)
  addExtra(ctrNode, ctrHeight)

  for _, item in ipairs(extras) do
    if leftHeight <= rightHeight then
      table.insert(leftCol, item.node)
      leftHeight += item.height + 12
    else
      table.insert(rightCol, item.node)
      rightHeight += item.height + 12
    end
  end

  table.insert(scrollChildren, ui.row({ gap = 14, align = "start" }, {
    ui.column({ gap = 12, flexGrow = 1 }, leftCol),
    ui.column({ gap = 12, flexGrow = 1 }, rightCol),
  }))

  panel.render(ui.column({ flexGrow = 1, gap = 12 }, {
    header,
    ui.scroll({ key = "metrics-scroll", flexGrow = 1, gap = 12 }, scrollChildren),
  }))
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
