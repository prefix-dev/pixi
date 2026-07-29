(() => {
  "use strict";

  let activeController = null;

  const initializeSignalLanding = () => {
    const landing = document.querySelector("[data-signal-landing]");

    if (!landing) {
      if (activeController) activeController.abort();
      activeController = null;
      return;
    }

    if (landing.dataset.signalInitialized === "true") return;
    if (activeController) activeController.abort();

    const controller = new AbortController();
    activeController = controller;
    const listenerOptions = { signal: controller.signal };

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
