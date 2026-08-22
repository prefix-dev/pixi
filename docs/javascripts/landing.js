(() => {
  "use strict";

  let activeController = null;

  // Jigsaw geometry for the capability grid: a knob is a single head circle
  // wrapped the long way round, blended into the edge by two concave fillet
  // arcs whose centers sit off the edge on the tab side — one bump, no lobes.
  // All sizes are in CSS pixels so knobs never distort with the card size, and
  // both pieces of a shared edge derive the same curve, so tabs mate exactly.
  const KNOB_HEAD_RADIUS = 13;
  const KNOB_FILLET_RADIUS = 7;
  const KNOB_HEAD_DISTANCE = 20;
  const PUZZLE_CORNER_RADIUS = 22;
  const PUZZLE_ARC_STEP = Math.PI / 14;

  const appendArc = (points, center, from, to, longWay) => {
    const startAngle = Math.atan2(from.y - center.y, from.x - center.x);
    let delta = Math.atan2(to.y - center.y, to.x - center.x) - startAngle;
    while (delta > Math.PI) delta -= 2 * Math.PI;
    while (delta < -Math.PI) delta += 2 * Math.PI;
    if (longWay) delta -= Math.sign(delta || 1) * 2 * Math.PI;
    const radius = Math.hypot(from.x - center.x, from.y - center.y);
    const steps = Math.max(4, Math.ceil(Math.abs(delta) / PUZZLE_ARC_STEP));
    for (let step = 1; step <= steps; step += 1) {
      const angle = startAngle + (delta * step) / steps;
      points.push({ x: center.x + radius * Math.cos(angle), y: center.y + radius * Math.sin(angle) });
    }
  };

  // Deterministic per-seam noise so both pieces of a shared edge agree on the
  // knob's size and placement while every seam looks a little different.
  const seamNoise = (seamRow, seamCol, orientation, salt) => {
    const value = Math.sin(seamRow * 127.1 + seamCol * 311.7 + orientation * 74.7 + salt * 269.5) * 43758.5453;
    return value - Math.floor(value);
  };

  const appendKnob = (points, edgeStart, edgeEnd, outward, along, headRadius, filletRadius, headDistance) => {
    const edgeX = edgeEnd.x - edgeStart.x;
    const edgeY = edgeEnd.y - edgeStart.y;
    const length = Math.hypot(edgeX, edgeY);
    const unitX = edgeX / length;
    const unitY = edgeY / length;
    const center = { x: edgeStart.x + unitX * length * along, y: edgeStart.y + unitY * length * along };
    const reach = headRadius + filletRadius;
    const rise = headDistance - filletRadius;
    const filletOffset = Math.sqrt(reach * reach - rise * rise);
    const head = { x: center.x + outward.x * headDistance, y: center.y + outward.y * headDistance };
    // Fillet centers sit off the edge on the tab side, tangent to the edge at
    // entry/exit, so the outline curves concavely into the neck.
    const nearFillet = {
      x: center.x - unitX * filletOffset + outward.x * filletRadius,
      y: center.y - unitY * filletOffset + outward.y * filletRadius,
    };
    const farFillet = {
      x: center.x + unitX * filletOffset + outward.x * filletRadius,
      y: center.y + unitY * filletOffset + outward.y * filletRadius,
    };
    const tangencyPoint = (fillet) => {
      const towardX = head.x - fillet.x;
      const towardY = head.y - fillet.y;
      const distance = Math.hypot(towardX, towardY);
      return {
        x: fillet.x + (filletRadius * towardX) / distance,
        y: fillet.y + (filletRadius * towardY) / distance,
      };
    };
    const entry = { x: center.x - unitX * filletOffset, y: center.y - unitY * filletOffset };
    const exit = { x: center.x + unitX * filletOffset, y: center.y + unitY * filletOffset };
    const nearTangency = tangencyPoint(nearFillet);
    const farTangency = tangencyPoint(farFillet);
    points.push(entry);
    appendArc(points, nearFillet, entry, nearTangency, false);
    appendArc(points, head, nearTangency, farTangency, true);
    appendArc(points, farFillet, farTangency, exit, false);
    points.push(exit);
  };

  const buildPiecePath = (row, col, rows, cols, width, height) => {
    const corners = {
      topLeft: row === 0 && col === 0 ? PUZZLE_CORNER_RADIUS : 0,
      topRight: row === 0 && col === cols - 1 ? PUZZLE_CORNER_RADIUS : 0,
      bottomRight: row === rows - 1 && col === cols - 1 ? PUZZLE_CORNER_RADIUS : 0,
      bottomLeft: row === rows - 1 && col === 0 ? PUZZLE_CORNER_RADIUS : 0,
    };
    // Tab/socket parity: horizontal seams alternate by (row + col), vertical
    // seams by column, so both sides of every seam agree on who owns the tab.
    const edges = [
      {
        from: { x: 0, y: 0 },
        to: { x: width, y: 0 },
        outward: { x: 0, y: -1 },
        kind: row === 0 ? "flat" : ((row - 1 + col) % 2 === 0 ? "tab" : "socket"),
        seam: { row: row - 1, col, orientation: 0 },
        canonical: true,
        startInset: corners.topLeft,
        endInset: corners.topRight,
        cornerCenter: { x: width - corners.topRight, y: corners.topRight },
        cornerRadius: corners.topRight,
      },
      {
        from: { x: width, y: 0 },
        to: { x: width, y: height },
        outward: { x: 1, y: 0 },
        kind: col === cols - 1 ? "flat" : ((row + col) % 2 === 0 ? "tab" : "socket"),
        seam: { row, col, orientation: 1 },
        canonical: true,
        startInset: corners.topRight,
        endInset: corners.bottomRight,
        cornerCenter: { x: width - corners.bottomRight, y: height - corners.bottomRight },
        cornerRadius: corners.bottomRight,
      },
      {
        from: { x: width, y: height },
        to: { x: 0, y: height },
        outward: { x: 0, y: 1 },
        kind: row === rows - 1 ? "flat" : ((row + col) % 2 === 0 ? "socket" : "tab"),
        seam: { row, col, orientation: 0 },
        canonical: false,
        startInset: corners.bottomRight,
        endInset: corners.bottomLeft,
        cornerCenter: { x: corners.bottomLeft, y: height - corners.bottomLeft },
        cornerRadius: corners.bottomLeft,
      },
      {
        from: { x: 0, y: height },
        to: { x: 0, y: 0 },
        outward: { x: -1, y: 0 },
        kind: col === 0 ? "flat" : ((row + col - 1) % 2 === 0 ? "socket" : "tab"),
        seam: { row, col: col - 1, orientation: 1 },
        canonical: false,
        startInset: corners.bottomLeft,
        endInset: corners.topLeft,
        cornerCenter: { x: corners.topLeft, y: corners.topLeft },
        cornerRadius: corners.topLeft,
      },
    ];

    const points = [];
    edges.forEach((edge) => {
      const unitX = Math.sign(edge.to.x - edge.from.x);
      const unitY = Math.sign(edge.to.y - edge.from.y);
      points.push({ x: edge.from.x + unitX * edge.startInset, y: edge.from.y + unitY * edge.startInset });
      if (edge.kind !== "flat") {
        const outward = edge.kind === "tab"
          ? edge.outward
          : { x: -edge.outward.x, y: -edge.outward.y };
        // Size and position vary per seam but are derived from the seam's grid
        // identity, so the two pieces sharing it always agree. `along` roams
        // the full corner-safe span of the edge and is measured left-to-right /
        // top-to-bottom; flip it for edges walked the opposite way.
        const scale = 0.78 + 0.4 * seamNoise(edge.seam.row, edge.seam.col, edge.seam.orientation, 1);
        const edgeLength = Math.hypot(edge.to.x - edge.from.x, edge.to.y - edge.from.y);
        const reach = (KNOB_HEAD_RADIUS + KNOB_FILLET_RADIUS) * scale;
        const rise = (KNOB_HEAD_DISTANCE - KNOB_FILLET_RADIUS) * scale;
        const footprint = Math.sqrt(reach * reach - rise * rise);
        const margin = Math.min(0.45, (PUZZLE_CORNER_RADIUS + footprint + 4) / edgeLength);
        const along = margin + (1 - 2 * margin) * seamNoise(edge.seam.row, edge.seam.col, edge.seam.orientation, 2);
        appendKnob(
          points,
          edge.from,
          edge.to,
          outward,
          edge.canonical ? along : 1 - along,
          KNOB_HEAD_RADIUS * scale,
          KNOB_FILLET_RADIUS * scale,
          KNOB_HEAD_DISTANCE * scale,
        );
      }
      const edgeEnd = { x: edge.to.x - unitX * edge.endInset, y: edge.to.y - unitY * edge.endInset };
      points.push(edgeEnd);
      if (edge.cornerRadius > 0) {
        // Quarter arc from this edge's end to the next edge's start.
        const followingIndex = (edges.indexOf(edge) + 1) % edges.length;
        const following = edges[followingIndex];
        const followingUnitX = Math.sign(following.to.x - following.from.x);
        const followingUnitY = Math.sign(following.to.y - following.from.y);
        const cornerExit = {
          x: following.from.x + followingUnitX * following.startInset,
          y: following.from.y + followingUnitY * following.startInset,
        };
        appendArc(points, edge.cornerCenter, edgeEnd, cornerExit, false);
      }
    });

    return `M ${points.map((point) => `${point.x.toFixed(2)} ${point.y.toFixed(2)}`).join(" L ")} Z`;
  };

  const initializeCapabilityPuzzle = (landing, controller, listenerOptions) => {
    const grid = landing.querySelector("[data-puzzle-grid]");
    if (!grid) return;
    const pieces = Array.from(grid.querySelectorAll("[data-puzzle-piece]"));
    if (!pieces.length) return;

    grid.classList.add("is-puzzle-active");

    const layoutPuzzle = () => {
      const columnPositions = new Set(pieces.map((piece) => Math.round(piece.offsetLeft)));
      const cols = Math.max(1, columnPositions.size);
      const rows = Math.ceil(pieces.length / cols);
      pieces.forEach((piece, index) => {
        const svg = piece.querySelector(".signal-capability__piece");
        const path = svg ? svg.querySelector("path") : null;
        const width = piece.offsetWidth;
        const height = piece.offsetHeight;
        if (!svg || !path || !width || !height) return;
        svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
        path.setAttribute("d", buildPiecePath(Math.floor(index / cols), index % cols, rows, cols, width, height));
      });
    };

    let puzzleFrame = null;
    const schedulePuzzleLayout = () => {
      if (controller.signal.aborted || puzzleFrame !== null) return;
      puzzleFrame = window.requestAnimationFrame(() => {
        puzzleFrame = null;
        layoutPuzzle();
      });
    };

    const puzzleObserver = typeof ResizeObserver === "function"
      ? new ResizeObserver(schedulePuzzleLayout)
      : null;
    if (puzzleObserver) [grid, ...pieces].forEach((element) => puzzleObserver.observe(element));
    window.addEventListener("resize", schedulePuzzleLayout, listenerOptions);
    controller.signal.addEventListener("abort", () => {
      puzzleObserver?.disconnect();
      if (puzzleFrame !== null) window.cancelAnimationFrame(puzzleFrame);
      puzzleFrame = null;
    }, { once: true });

    schedulePuzzleLayout();
    if (document.fonts?.ready) document.fonts.ready.then(schedulePuzzleLayout);
  };

  // Constellation nodes are loose puzzle pieces. All four edges carry a knob —
  // a centered tab facing the lockfile (where the connector anchors), a socket
  // on the outer edge, and one tab plus one socket on the sides — with bold
  // proportions so the silhouette reads as jigsaw at a glance.
  const NODE_KNOB_HEAD = 8;
  const NODE_KNOB_FILLET = 4;
  const NODE_KNOB_DISTANCE = 11.5;
  const NODE_CORNER_RADIUS = 10;

  const buildNodePiecePath = (width, height, tier, index, sideGap) => {
    const sizeFactor = Math.max(0.7, Math.min(1.25, Math.min(width, height) / 80));
    // When neighboring pieces sit close (mobile), side tabs would overlap them;
    // fall back to sockets, which carve inward instead.
    const maxProtrusion = (NODE_KNOB_DISTANCE + NODE_KNOB_HEAD) * 1.18 * sizeFactor;
    const allowSideTabs = sideGap > 2 * maxProtrusion + 4;
    const noise = (salt) => seamNoise(index + 1, 7.3, 2, salt);
    const radius = NODE_CORNER_RADIUS;
    const knobFootprint = (scale) => {
      const reach = (NODE_KNOB_HEAD + NODE_KNOB_FILLET) * scale;
      const rise = (NODE_KNOB_DISTANCE - NODE_KNOB_FILLET) * scale;
      return Math.sqrt(reach * reach - rise * rise);
    };
    const knob = (kind, edgeLength, alongNoise, scaleNoise, centered) => {
      if (kind === "flat") return null;
      const scale = (0.82 + 0.36 * scaleNoise) * sizeFactor;
      const margin = Math.min(0.45, (radius + knobFootprint(scale) + 2) / edgeLength);
      const along = centered ? 0.5 : margin + (1 - 2 * margin) * alongNoise;
      return { kind, along, scale };
    };
    // The lock-facing edge is always a centered tab (the connector anchors on
    // it); every other edge independently draws tab, socket, or flat, so the
    // pieces carry different knob counts like pieces from different parts of a
    // puzzle.
    const pickKind = (flatSalt, kindSalt, allowTab) => {
      if (noise(flatSalt) < 0.28) return "flat";
      return allowTab && noise(kindSalt) < 0.5 ? "tab" : "socket";
    };
    const lockSide = tier === "bottom" ? "top" : "bottom";
    const outerSide = lockSide === "top" ? "bottom" : "top";
    const kinds = {
      [lockSide]: "tab",
      [outerSide]: pickKind(15, 11, true),
      left: allowSideTabs ? pickKind(16, 6, true) : "flat",
      right: allowSideTabs ? pickKind(17, 12, true) : "flat",
    };
    // Never let a piece degrade to a lone tab on a rounded rectangle.
    if (kinds[outerSide] === "flat" && kinds.left === "flat" && kinds.right === "flat") {
      kinds[outerSide] = noise(11) < 0.5 ? "tab" : "socket";
    }
    const knobs = {
      top: knob(kinds.top, width, noise(4), noise(5), lockSide === "top"),
      bottom: knob(kinds.bottom, width, noise(14), noise(3), lockSide === "bottom"),
      left: knob(kinds.left, height, noise(7), noise(8), false),
      right: knob(kinds.right, height, noise(9), noise(10), false),
    };
    const edgeNormals = {
      top: { x: 0, y: -1 },
      right: { x: 1, y: 0 },
      bottom: { x: 0, y: 1 },
      left: { x: -1, y: 0 },
    };
    const edgeKnob = (points, edge, from, to, flipAlong) => {
      const spec = knobs[edge];
      if (!spec) return;
      const normal = edgeNormals[edge];
      const outward = spec.kind === "tab" ? normal : { x: -normal.x, y: -normal.y };
      const along = flipAlong ? 1 - spec.along : spec.along;
      appendKnob(points, from, to, outward, along, NODE_KNOB_HEAD * spec.scale, NODE_KNOB_FILLET * spec.scale, NODE_KNOB_DISTANCE * spec.scale);
    };

    const points = [{ x: radius, y: 0 }];
    edgeKnob(points, "top", { x: 0, y: 0 }, { x: width, y: 0 }, false);
    points.push({ x: width - radius, y: 0 });
    appendArc(points, { x: width - radius, y: radius }, { x: width - radius, y: 0 }, { x: width, y: radius }, false);
    edgeKnob(points, "right", { x: width, y: 0 }, { x: width, y: height }, false);
    points.push({ x: width, y: height - radius });
    appendArc(points, { x: width - radius, y: height - radius }, { x: width, y: height - radius }, { x: width - radius, y: height }, false);
    edgeKnob(points, "bottom", { x: width, y: height }, { x: 0, y: height }, true);
    points.push({ x: radius, y: height });
    appendArc(points, { x: radius, y: height - radius }, { x: radius, y: height }, { x: 0, y: height - radius }, false);
    edgeKnob(points, "left", { x: 0, y: height }, { x: 0, y: 0 }, true);
    points.push({ x: 0, y: radius });
    appendArc(points, { x: radius, y: radius }, { x: 0, y: radius }, { x: radius, y: 0 }, false);
    return `M ${points.map((point) => `${point.x.toFixed(2)} ${point.y.toFixed(2)}`).join(" L ")} Z`;
  };

  const initializeNodePieces = (landing, controller, listenerOptions) => {
    const map = landing.querySelector("[data-signal-map]");
    if (!map) return;
    const nodes = Array.from(map.querySelectorAll(".signal-node"));
    if (!nodes.length) return;

    map.classList.add("is-node-pieces");

    const layoutNodePieces = () => {
      nodes.forEach((node, index) => {
        const svg = node.querySelector(".signal-node__piece");
        const path = svg ? svg.querySelector("path") : null;
        const width = node.offsetWidth;
        const height = node.offsetHeight;
        if (!svg || !path || !width || !height) return;
        const tier = node.dataset.pieceTier === "bottom" ? "bottom" : "top";
        const sideGap = map.clientWidth * 0.25 - width;
        svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
        path.setAttribute("d", buildNodePiecePath(width, height, tier, index, sideGap));
        node.style.setProperty("--piece-tilt", `${((seamNoise(index + 1, 7.3, 2, 13) - 0.5) * 5).toFixed(2)}deg`);
      });
    };

    let nodeFrame = null;
    const scheduleNodeLayout = () => {
      if (controller.signal.aborted || nodeFrame !== null) return;
      nodeFrame = window.requestAnimationFrame(() => {
        nodeFrame = null;
        layoutNodePieces();
      });
    };

    const nodeObserver = typeof ResizeObserver === "function"
      ? new ResizeObserver(scheduleNodeLayout)
      : null;
    if (nodeObserver) nodes.forEach((node) => nodeObserver.observe(node));
    window.addEventListener("resize", scheduleNodeLayout, listenerOptions);
    controller.signal.addEventListener("abort", () => {
      nodeObserver?.disconnect();
      if (nodeFrame !== null) window.cancelAnimationFrame(nodeFrame);
      nodeFrame = null;
    }, { once: true });

    scheduleNodeLayout();
    if (document.fonts?.ready) document.fonts.ready.then(scheduleNodeLayout);
  };

  const initializeInstallCopy = (landing, listenerOptions) => {
    const button = landing.querySelector("[data-install-copy]");
    const status = landing.querySelector("[data-install-copy-status]");
    if (!button || !navigator.clipboard) return;
    let resetTimer = null;
    button.addEventListener("click", () => {
      navigator.clipboard.writeText(button.dataset.command || "").then(() => {
        button.classList.add("is-copied");
        if (status) status.textContent = "Install command copied to clipboard.";
        if (resetTimer !== null) window.clearTimeout(resetTimer);
        resetTimer = window.setTimeout(() => {
          resetTimer = null;
          button.classList.remove("is-copied");
          if (status) status.textContent = "";
        }, 1800);
      }).catch(() => {});
    }, listenerOptions);
  };

  const initializeGitHubStars = (landing, controller) => {
    const target = landing.querySelector("[data-github-stars]");
    if (!target || typeof window.fetch !== "function") return;
    const cacheKey = "signal-github-stars";
    const render = (count) => {
      if (!Number.isFinite(count) || controller.signal.aborted) return;
      const compact = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 }).format(count);
      target.textContent = `★ ${compact}`;
      target.hidden = false;
    };
    try {
      const cached = JSON.parse(window.sessionStorage.getItem(cacheKey) || "null");
      if (cached && Date.now() - cached.at < 3600000) {
        render(cached.count);
        return;
      }
    } catch { /* stale or unavailable cache is fine */ }
    window.fetch("https://api.github.com/repos/prefix-dev/pixi", { signal: controller.signal })
      .then((response) => (response.ok ? response.json() : null))
      .then((data) => {
        const count = data?.stargazers_count;
        if (!Number.isFinite(count)) return;
        try {
          window.sessionStorage.setItem(cacheKey, JSON.stringify({ at: Date.now(), count }));
        } catch { /* private mode */ }
        render(count);
      })
      .catch(() => {});
  };

  const initializeSignalLanding = () => {
    const landing = document.querySelector("[data-signal-landing]");

    if (!landing) {
      if (activeController) activeController.abort();
      activeController = null;
      return;
    }

    // Prototype theme switcher: ?theme=prefix|gradients|variant restyles the
    // page via scoped overrides in themes.css.
    const themeParam = new URLSearchParams(window.location.search).get("theme");
    if (["prefix", "gradients", "variant"].includes(themeParam)) {
      document.body.dataset.signalTheme = themeParam;
    }

    if (landing.dataset.signalInitialized === "true") return;
    if (activeController) activeController.abort();

    const controller = new AbortController();
    activeController = controller;
    const listenerOptions = { signal: controller.signal };

    initializeCapabilityPuzzle(landing, controller, listenerOptions);
    initializeNodePieces(landing, controller, listenerOptions);
    initializeInstallCopy(landing, listenerOptions);
    initializeGitHubStars(landing, controller);

    const terminal = landing.querySelector(".signal-terminal");
    const command = landing.querySelector("[data-add-command]");
    const dependencyText = landing.querySelector("[data-dependencies]");
    const taskText = landing.querySelector("[data-task-command]");
    const technologyText = landing.querySelector("[data-technology-label]");
    const manifestPlatforms = landing.querySelector("[data-manifest-platforms]");
    const manifestDependencies = landing.querySelector("[data-manifest-dependencies]");
    const manifestTask = landing.querySelector("[data-manifest-task]");
    const status = landing.querySelector("[data-terminal-status]");
    const map = landing.querySelector("[data-signal-map]");
    const lockfile = landing.querySelector(".signal-lock");
    const connectorLayer = landing.querySelector("[data-signal-connectors]");
    const stackTabList = landing.querySelector(".signal-stack-tabs");
    const nodes = Array.from(landing.querySelectorAll(".signal-node[data-packages][data-task]"));
    const stackTabs = Array.from(landing.querySelectorAll("[data-stack-tab][data-label]"));
    const connectorPaths = Array.from(landing.querySelectorAll("[data-signal-connector]"));
    const connectorEndpoints = Array.from(landing.querySelectorAll("[data-signal-endpoint]"));

    if (!terminal || !command || !dependencyText || !taskText || !technologyText || !manifestPlatforms || !manifestDependencies || !manifestTask || !status || !map || !lockfile || !connectorLayer || !stackTabList || !nodes.length || stackTabs.length !== nodes.length + 1 || connectorPaths.length !== nodes.length || connectorEndpoints.length !== nodes.length) return;

    landing.dataset.signalInitialized = "true";

    const TIMING = Object.freeze({
      erase: 260,
      type: 620,
      initialIdleMin: 2200,
      initialIdleMax: 2800,
      repeatIdleMin: 3800,
      repeatIdleMax: 4400,
    });

    const defaultPackages = command.dataset.default || "cmake rust nodejs go lua";
    const defaultTask = taskText.textContent || "cargo run";
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
    const supportsHover = window.matchMedia("(hover: hover) and (pointer: fine)");
    let animationToken = 0;
    let animationFrame = null;
    let connectorFrame = null;
    let autoTimer = null;
    let pinnedNode = null;
    let hoveredNode = null;
    let focusedNode = null;
    let focusedTab = null;
    let shownNode = null;
    let desiredValues = null;

    const randomBetween = (minimum, maximum) => minimum + Math.random() * (maximum - minimum);
    const formatManifestDependencies = (packages) => packages
      .trim()
      .split(/\s+/)
      .filter(Boolean)
      .map((packageName) => `${packageName} = "*"`)
      .join("\n");
    const defaultManifestPlatforms = 'platforms = ["linux-64", "osx-arm64", "win-64"]';
    const pytorchManifestPlatforms = `platforms = [
  "linux-64",
  { name = "cuda-12", platform = "linux-64", cuda = "12.0" },
  { name = "cuda-13", platform = "linux-64", cuda = "13.0" },
]`;
    const pytorchManifestDependencies = `python = "3.12.*"
[target.linux-64.dependencies]
pytorch-cpu = "*"
[target.cuda-12.dependencies]
pytorch-gpu = "*"
cuda-version = "12.*"
[target.cuda-13.dependencies]
pytorch-gpu = "*"
cuda-version = "13.*"`;

    const valuesFor = (packages, node = null) => {
      const isPyTorch = node?.dataset.label === "PyTorch";
      return {
        packages,
        technology: node ? (node.dataset.title || node.dataset.label) : "",
        task: node?.dataset.task || defaultTask,
        manifestPlatforms: isPyTorch ? pytorchManifestPlatforms : defaultManifestPlatforms,
        manifestDependencies: isPyTorch ? pytorchManifestDependencies : formatManifestDependencies(packages),
        manifestTask: node?.dataset.task || defaultTask,
      };
    };

    const textTargets = {
      packages: dependencyText,
      technology: technologyText,
      task: taskText,
      manifestPlatforms,
      manifestDependencies,
      manifestTask,
    };

    const readValues = () => Object.fromEntries(
      Object.entries(textTargets).map(([key, element]) => [key, element.textContent || ""]),
    );

    const renderValues = (values) => {
      Object.entries(textTargets).forEach(([key, element]) => {
        element.textContent = values[key];
      });
    };

    desiredValues = valuesFor(defaultPackages);

    const clearAnimationFrame = () => {
      if (!animationFrame) return;
      window.cancelAnimationFrame(animationFrame.id);
      animationFrame.resolve(false);
      animationFrame = null;
    };

    const cancelAnimation = () => {
      animationToken += 1;
      clearAnimationFrame();
      terminal.classList.remove("is-editing");
      landing.classList.remove("is-title-editing");
    };

    const animateValues = (fromValues, toValues, duration, erase, token) => new Promise((resolve) => {
      let startedAt = null;

      const finish = (completed) => {
        animationFrame = null;
        resolve(completed);
      };

      const renderFrame = (timestamp) => {
        if (token !== animationToken || controller.signal.aborted) {
          finish(false);
          return;
        }

        if (startedAt === null) startedAt = timestamp;
        const progress = Math.min(1, (timestamp - startedAt) / duration);
        const frame = Object.fromEntries(Object.keys(textTargets).map((key) => {
          const source = erase ? fromValues[key] : toValues[key];
          const length = erase
            ? Math.ceil(source.length * (1 - progress))
            : Math.floor(source.length * progress);
          return [key, source.slice(0, length)];
        }));
        renderValues(frame);

        if (progress >= 1) {
          finish(true);
          return;
        }

        const id = window.requestAnimationFrame(renderFrame);
        animationFrame = { id, resolve };
      };

      const id = window.requestAnimationFrame(renderFrame);
      animationFrame = { id, resolve };
    });

    const cancelAutoTimer = () => {
      if (autoTimer === null) return;
      window.clearTimeout(autoTimer);
      autoTimer = null;
    };

    const interactionIsActive = () => Boolean(pinnedNode || hoveredNode || focusedNode || focusedTab);
    const nodesByLabel = new Map(nodes.map((node) => [node.dataset.label, node]));

    const connectorPoint = (node, tier) => ({
      x: node.offsetLeft,
      y: tier === "top"
        ? node.offsetTop + node.offsetHeight / 2
        : node.offsetTop - node.offsetHeight / 2,
    });

    const updateConnectorGeometry = () => {
      connectorFrame = null;
      const width = map.clientWidth;
      const height = map.clientHeight;
      if (!width || !height) return;

      connectorLayer.setAttribute("viewBox", `0 0 ${width} ${height}`);
      const pathsByTier = {
        top: connectorPaths.filter((path) => path.dataset.tier === "top"),
        bottom: connectorPaths.filter((path) => path.dataset.tier === "bottom"),
      };
      const lockEdgePoint = (path, tier) => {
        const tierPaths = pathsByTier[tier];
        const index = tierPaths.indexOf(path);
        const fraction = (index + 1) / (tierPaths.length + 1);
        return {
          x: lockfile.offsetLeft - lockfile.offsetWidth / 2 + lockfile.offsetWidth * fraction,
          y: tier === "top"
            ? lockfile.offsetTop - lockfile.offsetHeight / 2
            : lockfile.offsetTop + lockfile.offsetHeight / 2,
        };
      };

      connectorPaths.forEach((path) => {
        const node = nodesByLabel.get(path.dataset.node);
        const tier = path.dataset.tier;
        if (!node || (tier !== "top" && tier !== "bottom")) return;
        const nodePoint = connectorPoint(node, tier);
        const lockPoint = lockEdgePoint(path, tier);
        const start = tier === "top" ? nodePoint : lockPoint;
        const end = tier === "top" ? lockPoint : nodePoint;
        const bend = Math.max(20, (end.y - start.y) * 0.48);
        path.setAttribute("d", `M ${start.x} ${start.y} C ${start.x} ${start.y + bend}, ${end.x} ${end.y - bend}, ${end.x} ${end.y}`);
      });

      connectorEndpoints.forEach((endpoint) => {
        const node = nodesByLabel.get(endpoint.dataset.node);
        if (!node) return;
        const point = connectorPoint(node, endpoint.dataset.tier);
        endpoint.setAttribute("cx", String(point.x));
        endpoint.setAttribute("cy", String(point.y));
      });
    };

    const scheduleConnectorGeometry = () => {
      if (controller.signal.aborted || connectorFrame !== null) return;
      connectorFrame = window.requestAnimationFrame(updateConnectorGeometry);
    };

    const connectorObserver = typeof ResizeObserver === "function"
      ? new ResizeObserver(scheduleConnectorGeometry)
      : null;
    if (connectorObserver) [map, lockfile, ...nodes].forEach((element) => connectorObserver.observe(element));
    window.addEventListener("resize", scheduleConnectorGeometry, listenerOptions);
    scheduleConnectorGeometry();
    if (document.fonts?.ready) document.fonts.ready.then(scheduleConnectorGeometry);

    const markShownNode = (node) => {
      nodes.forEach((item) => item.classList.toggle("is-active", item === node));
      let activeTab = null;
      stackTabs.forEach((tab, index) => {
        const selected = node
          ? tab.dataset.label === node.dataset.label
          : tab.hasAttribute("data-default-stack");
        tab.setAttribute("aria-selected", String(selected));
        tab.tabIndex = selected || (!node && index === 0) ? 0 : -1;
        if (selected) activeTab = tab;
      });
      if (activeTab) {
        const targetLeft = activeTab.offsetLeft + activeTab.offsetWidth / 2 - stackTabList.clientWidth / 2;
        stackTabList.scrollTo({
          left: Math.max(0, Math.min(targetLeft, stackTabList.scrollWidth - stackTabList.clientWidth)),
          behavior: reducedMotion.matches ? "auto" : "smooth",
        });
      }
      shownNode = node;
    };

    const applyImmediately = () => {
      renderValues(desiredValues);
      landing.classList.toggle("has-stack-selection", Boolean(desiredValues.technology));
      terminal.classList.remove("is-editing");
      landing.classList.remove("is-title-editing");
    };

    const updateDisplays = async (packages, node = null, announce = false) => {
      const nextValues = valuesFor(packages, node);
      const currentValues = readValues();
      const valuesMatch = Object.keys(textTargets).every((key) => currentValues[key] === nextValues[key]);
      if (valuesMatch && shownNode === node) {
        if (announce && node) status.textContent = `Workspace examples updated for ${node.dataset.label}.`;
        return;
      }

      cancelAnimation();
      const token = animationToken;
      desiredValues = nextValues;
      markShownNode(node);
      if (announce) status.textContent = "";
      landing.classList.toggle("has-stack-selection", Boolean(nextValues.technology));

      if (reducedMotion.matches) {
        applyImmediately();
        if (announce && node) status.textContent = `Workspace examples updated for ${node.dataset.label}.`;
        return;
      }

      terminal.classList.add("is-editing");
      landing.classList.add("is-title-editing");

      const erased = await animateValues(currentValues, nextValues, TIMING.erase, true, token);
      if (!erased) return;
      const typed = await animateValues(currentValues, nextValues, TIMING.type, false, token);
      if (!typed) return;

      renderValues(nextValues);
      terminal.classList.remove("is-editing");
      landing.classList.remove("is-title-editing");
      if (announce && node) status.textContent = `Workspace examples updated for ${node.dataset.label}.`;
    };

    const showNode = (node, announce = true) => updateDisplays(node.dataset.packages, node, announce);
    const showMixedStack = async (announce = true) => {
      await updateDisplays(defaultPackages, null, false);
      if (announce) status.textContent = "Workspace examples updated for the Mixed stack.";
    };

    const chooseDifferentNode = () => {
      const choices = nodes.filter((node) => node !== shownNode);
      return choices[Math.floor(Math.random() * choices.length)];
    };

    const scheduleAutoSelection = (initial = true) => {
      cancelAutoTimer();
      if (reducedMotion.matches || document.hidden || interactionIsActive() || controller.signal.aborted) return;

      const delay = initial
        ? randomBetween(TIMING.initialIdleMin, TIMING.initialIdleMax)
        : randomBetween(TIMING.repeatIdleMin, TIMING.repeatIdleMax);
      autoTimer = window.setTimeout(() => {
        autoTimer = null;
        if (reducedMotion.matches || document.hidden || interactionIsActive() || controller.signal.aborted) return;
        showNode(chooseDifferentNode(), false);
        scheduleAutoSelection(false);
      }, delay);
    };

    const resumeWhenIdle = () => {
      if (!interactionIsActive()) scheduleAutoSelection(true);
    };

    const setPinnedNode = (node) => {
      cancelAutoTimer();
      pinnedNode = node;
      nodes.forEach((item) => item.setAttribute("aria-pressed", String(item === node)));
      if (node) showNode(node, true);
      else if (focusedNode) showNode(focusedNode, true);
      else if (hoveredNode) showNode(hoveredNode, true);
      else if (focusedTab?.hasAttribute("data-default-stack")) showMixedStack(true);
      else if (focusedTab) showNode(nodesByLabel.get(focusedTab.dataset.label), true);
      else resumeWhenIdle();
    };

    nodes.forEach((node) => {
      node.addEventListener("pointerdown", cancelAutoTimer, listenerOptions);

      node.addEventListener("pointerenter", () => {
        if (!supportsHover.matches) return;
        cancelAutoTimer();
        hoveredNode = node;
        if (!pinnedNode) showNode(node, true);
      }, listenerOptions);

      node.addEventListener("pointerleave", () => {
        if (!supportsHover.matches) return;
        hoveredNode = null;
        if (pinnedNode) showNode(pinnedNode, true);
        else if (focusedNode) showNode(focusedNode, true);
        else resumeWhenIdle();
      }, listenerOptions);

      node.addEventListener("focus", () => {
        cancelAutoTimer();
        focusedNode = node;
        if (!pinnedNode) showNode(node, true);
      }, listenerOptions);

      node.addEventListener("blur", () => {
        focusedNode = null;
        if (pinnedNode) showNode(pinnedNode, true);
        else if (hoveredNode) showNode(hoveredNode, true);
        else resumeWhenIdle();
      }, listenerOptions);

      node.addEventListener("click", () => {
        const shouldReset = pinnedNode === node;
        setPinnedNode(shouldReset ? null : node);
        if (shouldReset && !supportsHover.matches) node.blur();
      }, listenerOptions);

      node.addEventListener("keydown", (event) => {
        if (event.key !== "Escape") return;
        event.preventDefault();
        setPinnedNode(null);
        node.blur();
      }, listenerOptions);
    });

    stackTabs.forEach((tab, index) => {
      const node = nodesByLabel.get(tab.dataset.label);

      tab.addEventListener("pointerdown", cancelAutoTimer, listenerOptions);

      tab.addEventListener("focus", () => {
        cancelAutoTimer();
        focusedTab = tab;
      }, listenerOptions);

      tab.addEventListener("blur", () => {
        focusedTab = null;
        if (!pinnedNode) resumeWhenIdle();
      }, listenerOptions);

      tab.addEventListener("click", () => {
        if (node) {
          setPinnedNode(node);
          return;
        }
        cancelAutoTimer();
        pinnedNode = null;
        nodes.forEach((item) => item.setAttribute("aria-pressed", "false"));
        showMixedStack(true);
      }, listenerOptions);

      tab.addEventListener("keydown", (event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          setPinnedNode(null);
          tab.blur();
          return;
        }

        const directions = { ArrowLeft: -1, ArrowRight: 1 };
        let nextIndex = directions[event.key] === undefined
          ? null
          : (index + directions[event.key] + stackTabs.length) % stackTabs.length;
        if (event.key === "Home") nextIndex = 0;
        if (event.key === "End") nextIndex = stackTabs.length - 1;
        if (nextIndex === null) return;
        event.preventDefault();
        stackTabs[nextIndex].focus();
      }, listenerOptions);
    });

    document.addEventListener("pointerdown", (event) => {
      if (pinnedNode && !map.contains(event.target) && !stackTabList.contains(event.target)) setPinnedNode(null);
    }, listenerOptions);

    document.addEventListener("visibilitychange", () => {
      if (document.hidden) cancelAutoTimer();
      else resumeWhenIdle();
    }, listenerOptions);

    reducedMotion.addEventListener("change", () => {
      cancelAutoTimer();
      if (reducedMotion.matches) {
        cancelAnimation();
        applyImmediately();
      } else {
        resumeWhenIdle();
      }
    }, listenerOptions);

    controller.signal.addEventListener("abort", () => {
      cancelAutoTimer();
      cancelAnimation();
      connectorObserver?.disconnect();
      if (connectorFrame !== null) window.cancelAnimationFrame(connectorFrame);
      connectorFrame = null;
    }, { once: true });

    scheduleAutoSelection(true);
  };

  if (typeof document$ !== "undefined") {
    document$.subscribe(initializeSignalLanding);
  } else if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", initializeSignalLanding, { once: true });
  } else {
    initializeSignalLanding();
  }
})();
