const API_BASE = "http://127.0.0.1:7330";

const canvas = document.getElementById("graphCanvas");
const details = document.getElementById("nodeDetails");
const nodeCount = document.getElementById("nodeCount");
const edgeCount = document.getElementById("edgeCount");
const kindFilters = document.getElementById("kindFilters");
const edgeLegend = document.getElementById("edgeLegend");
const searchInput = document.getElementById("searchInput");
const analyzeTargetInput = document.getElementById("analyzeTargetInput");
const analyzeTargetButton = document.getElementById("analyzeTargetButton");
const filterInput = document.getElementById("filterInput");
const resetFilter = document.getElementById("resetFilter");
const fitButton = document.getElementById("fitButton");
const exportButton = document.getElementById("exportButton");
const focusDepthInput = document.getElementById("focusDepth");
const searchResults = document.getElementById("searchResults");
const eventFeed = document.getElementById("eventFeed");
const eventSummary = document.getElementById("eventSummary");
const snapshotFeed = document.getElementById("snapshotFeed");
const snapshotSummary = document.getElementById("snapshotSummary");
const snapshotCount = document.getElementById("snapshotCount");
const adapterFeed = document.getElementById("adapterFeed");
const adapterSummary = document.getElementById("adapterSummary");
const memoryFeed = document.getElementById("memoryFeed");
const memorySummary = document.getElementById("memorySummary");
const sessionFeed = document.getElementById("sessionFeed");
const sessionSummary = document.getElementById("sessionSummary");
const embeddingFeed = document.getElementById("embeddingFeed");
const embeddingSummary = document.getElementById("embeddingSummary");
const embeddingSearchInput = document.getElementById("embeddingSearchInput");
const embeddingSearchButton = document.getElementById("embeddingSearchButton");
const inspectorPane = document.getElementById("inspectorPane");
const navigatorPane = document.getElementById("navigatorPane");
const navigatorTraversal = document.getElementById("navigatorTraversal");
const navigatorTraversalCount = document.getElementById("navigatorTraversalCount");
const navigatorList = document.getElementById("navigatorList");
const navigatorCount = document.getElementById("navigatorCount");
const railButtons = Array.from(document.querySelectorAll(".rail-button[data-surface]"));
const DASHBOARD_STORAGE_KEY = "memorycore.dashboard";
const SURFACES = new Set(["graph", "files", "plugins", "skills", "adapters", "memory", "sessions"]);

function readDashboardState() {
  const fallback = { depth: 2, node: null, tab: "inspector", surface: "graph" };
  try {
    const url = new URL(window.location.href);
    const depth = Number.parseInt(url.searchParams.get("depth") || "", 10);
    const node = url.searchParams.get("node");
    const tab = url.searchParams.get("tab");
    const surface = url.searchParams.get("surface");
    const stored = JSON.parse(localStorage.getItem(DASHBOARD_STORAGE_KEY) || "{}");
    return {
      depth: Number.isFinite(depth) ? Math.min(8, Math.max(0, depth)) : Number.isFinite(Number.parseInt(stored.depth, 10)) ? Math.min(8, Math.max(0, Number.parseInt(stored.depth, 10))) : fallback.depth,
      node: node || (typeof stored.node === "string" ? stored.node : fallback.node),
      tab: tab === "navigator" || tab === "inspector" ? tab : (stored.tab === "navigator" || stored.tab === "inspector" ? stored.tab : fallback.tab),
      surface: SURFACES.has(surface)
        ? surface
        : (SURFACES.has(stored.surface)
          ? stored.surface
          : fallback.surface),
    };
  } catch (_) {
    return fallback;
  }
}

function persistDashboardState(nextState = {}) {
  const state = {
    depth: Number.isFinite(Number.parseInt(nextState.depth ?? focusDepth, 10))
      ? Math.min(8, Math.max(0, Number.parseInt(nextState.depth ?? focusDepth, 10)))
      : focusDepth,
    node: typeof nextState.node === "string" ? nextState.node : nextState.node === null ? null : selectedId,
    tab: nextState.tab === "navigator" || nextState.tab === "inspector" ? nextState.tab : activeTab,
    surface: SURFACES.has(nextState.surface)
      ? nextState.surface
      : activeSurface,
  };
  try {
    localStorage.setItem(DASHBOARD_STORAGE_KEY, JSON.stringify(state));
  } catch (_) {
    /* ignore storage failures */
  }
  try {
    const url = new URL(window.location.href);
    url.searchParams.set("depth", `${state.depth}`);
    if (state.node) url.searchParams.set("node", state.node);
    else url.searchParams.delete("node");
    url.searchParams.set("tab", state.tab);
    url.searchParams.set("surface", state.surface);
    history.replaceState({}, "", `${url.pathname}${url.search}${url.hash}`);
  } catch (_) {
    /* ignore history failures */
  }
}

const dashboardState = readDashboardState();
let focusDepth = dashboardState.depth;
let initialSelectedId = dashboardState.node;
let activeTab = dashboardState.tab;
let activeSurface = dashboardState.surface;
if (focusDepthInput) {
  focusDepthInput.value = `${focusDepth}`;
}
setActiveTab(activeTab);
setActiveSurface(activeSurface);

const sampleGraph = {
  nodes: [
    { id: "project:root", kind: "Project", name: "memorycore", path: "." },
    { id: "folder:crates", kind: "Folder", name: "crates", path: "crates" },
    {
      id: "file:crates/memorycore-cli/src/main.rs",
      kind: "File",
      name: "main.rs",
      path: "crates/memorycore-cli/src/main.rs",
    },
    {
      id: "file:crates/memorycore-cli/src/mcp.rs",
      kind: "File",
      name: "mcp.rs",
      path: "crates/memorycore-cli/src/mcp.rs",
    },
    {
      id: "memory:generate-diagram",
      kind: "MemoryCase",
      name: "generate-diagram",
      path: "skills/generate-diagram/SKILL.md",
    },
  ],
  edges: [
    { source: "project:root", target: "folder:crates", kind: "contains" },
    {
      source: "folder:crates",
      target: "file:crates/memorycore-cli/src/main.rs",
      kind: "contains",
    },
    {
      source: "file:crates/memorycore-cli/src/main.rs",
      target: "file:crates/memorycore-cli/src/mcp.rs",
      kind: "imports",
    },
    {
      source: "memory:generate-diagram",
      target: "file:crates/memorycore-cli/src/mcp.rs",
      kind: "explains",
    },
  ],
};

let graph = sampleGraph;
let nodeMap = new Map();
let selectedId = null;
let enabledKinds = new Set();
let view = { x: 0, y: 0, scale: 1 };
let drag = null;
let focusMode = false;
let selectedDetails = null;
let selectedDetailsLoading = false;
let selectedDetailsSeq = 0;
let selectedImpact = "";
let selectedImpactLoading = false;
let selectedImpactSeq = 0;
let selectedAnalysis = null;
let selectedAnalysisLoading = false;
let selectedAnalysisSeq = 0;
let selectedAnalysisTarget = "";
let selectedAnalysisMermaid = "";
let searchHits = [];
let searchSeq = 0;
let eventItems = [];
let eventSeq = 0;
let eventTotal = 0;
let snapshotItems = [];
let snapshotSeq = 0;
let snapshotTotal = 0;
let adapterItems = [];
let adapterSeq = 0;
let adapterTotal = 0;
let memoryItems = [];
let memorySeq = 0;
let memoryTotal = 0;
let sessionItems = [];
let sessionSeq = 0;
let sessionTotal = 0;
let embeddingItems = [];
let embeddingSeq = 0;
let embeddingTotal = 0;
let embeddingPath = "";
let embeddingSearchHits = [];
let embeddingSearchQuery = "";
let embeddingSearchSeq = 0;
let selectedEventItems = [];
let selectedEventSeq = 0;
let selectedEventLoading = false;
let selectedSnapshot = null;
let selectedSnapshotLoading = false;
let selectedSession = null;
let selectedSessionLoading = false;
let selectedSessionSeq = 0;
let refreshTimer = null;
let enabledEdgeKinds = new Set();
const REFRESH_MS = 5000;
let focusRequestSeq = 0;

function setActiveTab(tab) {
  const nextTab = tab === "navigator" ? "navigator" : "inspector";
  activeTab = nextTab;
  persistDashboardState({ tab: activeTab });
  for (const button of document.querySelectorAll(".tabs button")) {
    const isActive = button.dataset.tab === activeTab;
    button.classList.toggle("active", isActive);
  }
  if (inspectorPane) inspectorPane.hidden = activeTab !== "inspector";
  if (navigatorPane) navigatorPane.hidden = activeTab !== "navigator";
  renderNavigator();
}

function setActiveSurface(surface) {
  const nextSurface = SURFACES.has(surface) ? surface : "graph";
  activeSurface = nextSurface;
  persistDashboardState({ surface: activeSurface });
  for (const button of railButtons) {
    button.classList.toggle("active", button.dataset.surface === activeSurface);
  }
  renderNavigator();
}

function updateSurfaceCounts(counts = []) {
  const bySurface = new Map(counts.map((item) => [item.surface, item.count]));
  for (const button of railButtons) {
    const surface = button.dataset.surface;
    const count = bySurface.get(surface) ?? 0;
    const countNode = button.querySelector(".rail-count");
    if (countNode) {
      countNode.textContent = `${count}`;
    }
    const baseTitle = button.dataset.surfaceTitle || button.title || surface || "";
    button.title = `${baseTitle} (${count})`;
  }
}

function setFocusDepth(nextDepth, { refreshSelected = true } = {}) {
  const parsed = Number.parseInt(`${nextDepth}`, 10);
  if (!Number.isFinite(parsed)) return;
  const clamped = Math.min(8, Math.max(0, parsed));
  focusDepth = clamped;
  persistDashboardState({ depth: focusDepth });
  if (focusDepthInput && focusDepthInput.value !== `${clamped}`) {
    focusDepthInput.value = `${clamped}`;
  }
  if (!refreshSelected || !selectedId) {
    return;
  }
  if (focusMode) {
    void focusNode(selectedId);
    return;
  }
  void refreshSelectedDetails(selectedId);
  void refreshSelectedImpact(selectedId);
  void loadSelectedAnalysis(selectedId);
}

async function loadGraph({ preserveSelection = false } = {}) {
  try {
    const response = await fetch(`${API_BASE}/graph.json`, { cache: "no-store" });
    if (response.ok) {
      graph = await response.json();
      if (!preserveSelection) {
        focusMode = false;
        selectedId = null;
        selectedDetails = null;
        selectedDetailsLoading = false;
        selectedAnalysis = null;
        selectedAnalysisLoading = false;
        selectedAnalysisTarget = "";
        selectedAnalysisMermaid = "";
        selectedSession = null;
        selectedSessionLoading = false;
      }
    }
  } catch (_) {
    graph = sampleGraph;
  }
  rebuildGraphIndex();
  if (!preserveSelection) {
    enabledKinds = new Set(graph.nodes.map((node) => node.kind));
  } else {
    for (const node of graph.nodes) {
      enabledKinds.add(node.kind);
    }
  }
  hydrateFilters();
  layoutGraph();
  if (preserveSelection && focusMode && selectedId) {
    const selected = nodeMap.get(selectedId);
    if (selected) {
      selectedId = selected.id;
    }
  }
  if (selectedId) {
    void refreshSelectedDetails(selectedId);
    void refreshSelectedImpact(selectedId);
    void loadSelectedAnalysis(selectedId);
    void loadSelectedEvents(selectedId);
    void loadSelectedSession(selectedId);
  }
  render();
  void loadStatus();
  void loadEvents();
  void loadSnapshots();
  void loadAdapters();
  void loadMemoryCases();
  void loadSessions();
  void loadEmbeddings();
}

function rebuildGraphIndex() {
  nodeMap = new Map(graph.nodes.map((node) => [node.id, node]));
}

function mergeGraph(subgraph) {
  const merged = new Map(graph.nodes.map((node) => [node.id, node]));
  for (const node of subgraph.nodes || []) {
    enabledKinds.add(node.kind);
    merged.set(node.id, { ...merged.get(node.id), ...node });
  }
  const edgeKeys = new Set(graph.edges.map(edgeKey));
  const edges = [...graph.edges];
  for (const edge of subgraph.edges || []) {
    const key = edgeKey(edge);
    if (!edgeKeys.has(key)) {
      edgeKeys.add(key);
      edges.push(edge);
    }
  }
  graph = { nodes: [...merged.values()], edges };
  rebuildGraphIndex();
}

function edgeKey(edge) {
  return `${edge.source}::${edge.kind}::${edge.target}`;
}

function hydrateFilters() {
  nodeCount.textContent = graph.nodes.length;
  const edgeKinds = [...new Set(graph.edges.map((edge) => edge.kind))].sort();
  if (!enabledEdgeKinds.size) {
    enabledEdgeKinds = new Set(edgeKinds);
  } else {
    for (const kind of edgeKinds) {
      if (!enabledEdgeKinds.has(kind)) {
        enabledEdgeKinds.add(kind);
      }
    }
  }
  edgeCount.textContent = graph.edges.filter((edge) => enabledEdgeKinds.has(edge.kind)).length;

  const kinds = [...new Set(graph.nodes.map((node) => node.kind))].sort();
  kindFilters.innerHTML = "";
  for (const kind of kinds) {
    const count = graph.nodes.filter((node) => node.kind === kind).length;
    const row = document.createElement("div");
    row.className = "check-row";
    row.innerHTML = `<label><input type="checkbox" checked data-kind="${kind}">${kind}</label><span>${count}</span>`;
    kindFilters.appendChild(row);
  }

  edgeLegend.innerHTML = "";
  for (const kind of edgeKinds) {
    const count = graph.edges.filter((edge) => edge.kind === kind).length;
    const row = document.createElement("div");
    row.className = `legend-row${enabledEdgeKinds.has(kind) ? " active" : " muted"}`;
    row.dataset.kind = kind;
    row.tabIndex = 0;
    row.setAttribute("role", "button");
    row.setAttribute("aria-pressed", String(enabledEdgeKinds.has(kind)));
    row.innerHTML = `<span>${kind}</span><span>${count}</span>`;
    edgeLegend.appendChild(row);
  }

  for (const row of edgeLegend.querySelectorAll(".legend-row")) {
    row.addEventListener("click", () => {
      const kind = row.dataset.kind;
      if (!kind) return;
      if (enabledEdgeKinds.has(kind)) {
        enabledEdgeKinds.delete(kind);
      } else {
        enabledEdgeKinds.add(kind);
      }
      hydrateFilters();
      render();
    });
  }
}

function layoutGraph() {
  const width = Math.max(canvas.clientWidth, 800);
  const centerX = width / 2;
  const layers = new Map();
  for (const node of graph.nodes) {
    const depth = node.kind === "Project" ? 0 : node.kind === "Folder" ? 1 : node.kind === "File" ? 2 : 3;
    if (!layers.has(depth)) layers.set(depth, []);
    layers.get(depth).push(node);
  }
  for (const [depth, nodes] of layers.entries()) {
    nodes.forEach((node, index) => {
      node.x = centerX + (index - (nodes.length - 1) / 2) * 230;
      node.y = 80 + depth * 170;
    });
  }
}

function filteredNodes() {
  const query = `${searchInput.value} ${filterInput.value}`.trim().toLowerCase();
  return graph.nodes.filter((node) => {
    const haystack = `${node.id} ${node.kind} ${node.name} ${node.path || ""}`.toLowerCase();
    return enabledKinds.has(node.kind) && (!query || haystack.includes(query));
  });
}

function nodesForSurface() {
  const nodes = filteredNodes();
  switch (activeSurface) {
    case "files":
      return nodes.filter((node) => ["File", "Folder", "Project"].includes(node.kind));
    case "plugins":
      return nodes.filter((node) => node.kind === "Plugin");
    case "skills":
      return nodes.filter((node) => node.kind === "Skill");
    case "adapters":
      return nodes.filter((node) => node.kind === "Adapter");
    case "memory":
      return nodes.filter((node) => node.kind === "MemoryCase");
    case "sessions":
      return nodes.filter((node) => ["Session", "Message"].includes(node.kind));
    default:
      return nodes;
  }
}

function searchKindsForSurface() {
  switch (activeSurface) {
    case "files":
      return "File,Folder,Project";
    case "plugins":
      return "Plugin";
    case "skills":
      return "Skill";
    case "adapters":
      return "Adapter";
    case "memory":
      return "MemoryCase";
    case "sessions":
      return "Session,Message";
    default:
      return null;
  }
}

function render() {
  const nodes = filteredNodes();
  const nodeIds = new Set(nodes.map((node) => node.id));
  const edges = graph.edges.filter(
    (edge) => enabledEdgeKinds.has(edge.kind) && nodeIds.has(edge.source) && nodeIds.has(edge.target),
  );
  canvas.innerHTML = "";
  canvas.setAttribute(
    "viewBox",
    `${-view.x} ${-view.y} ${canvas.clientWidth / view.scale} ${canvas.clientHeight / view.scale}`,
  );

  const defs = svg("defs");
  defs.innerHTML = `<marker id="arrow" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto"><path d="M0,0 L8,4 L0,8 Z" fill="#8592a8"></path></marker>`;
  canvas.appendChild(defs);

  for (const edge of edges) {
    const source = nodeMap.get(edge.source);
    const target = nodeMap.get(edge.target);
    if (!source || !target) continue;
    const path = svg("path", {
      class: "edge",
      d: curve(source.x, source.y, target.x, target.y),
      "marker-end": "url(#arrow)",
    });
    canvas.appendChild(path);
    const label = svg("text", {
      class: "edge-label",
      x: (source.x + target.x) / 2,
      y: (source.y + target.y) / 2 - 8,
    });
    label.textContent = edge.kind;
    canvas.appendChild(label);
  }

  for (const node of nodes) {
    const group = svg("g", {
      class: `node kind-${node.kind}${node.id === selectedId ? " selected" : ""}`,
      transform: `translate(${node.x - 82}, ${node.y - 30})`,
    });
    group.innerHTML = `<rect width="164" height="60" rx="8"></rect><text x="14" y="25">${escapeHtml(node.name)}</text><text class="sub" x="14" y="43">${escapeHtml(node.path || node.id)}</text>`;
    group.addEventListener("click", (event) => {
      event.stopPropagation();
      selectNode(node.id);
    });
    group.addEventListener("dblclick", async (event) => {
      event.stopPropagation();
      await focusNode(node.id);
    });
    canvas.appendChild(group);
  }

  renderInspector();
  renderNavigator();
}

function selectNode(id) {
  selectedId = id;
  persistDashboardState({ node: id });
  selectedImpact = "";
  selectedImpactLoading = true;
  selectedAnalysis = null;
  selectedAnalysisLoading = true;
  selectedAnalysisMermaid = "";
  selectedEventItems = [];
  selectedEventLoading = true;
  selectedSnapshot = null;
  selectedSnapshotLoading = false;
  selectedSession = null;
  selectedSessionLoading = false;
  void loadSelectedSession(id);
  void refreshSelectedDetails(id);
  void refreshSelectedImpact(id);
  void loadSelectedAnalysis(id);
  void loadSelectedEvents(id);
  render();
}

async function focusNode(id) {
  const seq = ++focusRequestSeq;
  const depth = focusDepth;
  try {
    const response = await fetch(`${API_BASE}/graph/${encodeURIComponent(id)}?depth=${depth}`, {
      cache: "no-store",
    });
    if (response.ok) {
      const subset = await response.json();
      if (seq !== focusRequestSeq || depth !== focusDepth) return;
      const nodes = [];
      const seen = new Set();
      for (const node of subset.nodes || []) {
        if (node?.id && !seen.has(node.id)) {
          seen.add(node.id);
          nodes.push(node);
        }
      }
      if (subset.focus?.id && !seen.has(subset.focus.id)) {
        seen.add(subset.focus.id);
        nodes.push(subset.focus);
      }
      for (const edge of subset.edges || []) {
        for (const endpoint of [edge.source, edge.target]) {
          if (!endpoint || seen.has(endpoint)) continue;
          seen.add(endpoint);
          const existing = nodeMap.get(endpoint);
          nodes.push(
            existing || {
              id: endpoint,
              kind: "Node",
              name: endpoint.split(":").pop(),
              path: endpoint,
            },
          );
        }
      }
      mergeGraph({ nodes, edges: subset.edges || [] });
      focusMode = true;
      selectedId = id;
      persistDashboardState({ node: id });
      selectedImpact = "";
      selectedImpactLoading = true;
      selectedAnalysis = null;
      selectedAnalysisLoading = true;
      selectedAnalysisMermaid = "";
      selectedEventItems = [];
      selectedEventLoading = true;
      selectedSnapshot = null;
      selectedSnapshotLoading = false;
      selectedSession = null;
      selectedSessionLoading = false;
      selectedDetails = subset.focus
        ? { ...subset.focus, nodes: subset.nodes || [], edges: subset.edges || [] }
        : null;
      void refreshSelectedImpact(id);
      void loadSelectedAnalysis(id);
      void loadSelectedEvents(id);
      void loadSelectedSession(id);
      selectedDetailsLoading = false;
      hydrateFilters();
      layoutGraph();
      render();
    }
  } catch (_) {
    if (seq !== focusRequestSeq || depth !== focusDepth) return;
    selectedId = id;
    persistDashboardState({ node: id });
    selectedImpact = "";
    selectedImpactLoading = true;
    selectedAnalysis = null;
    selectedAnalysisLoading = true;
    selectedAnalysisMermaid = "";
    selectedEventItems = [];
    selectedEventLoading = true;
    selectedSnapshot = null;
    selectedSnapshotLoading = false;
    selectedSession = null;
    selectedSessionLoading = false;
    void refreshSelectedDetails(id);
    void refreshSelectedImpact(id);
    void loadSelectedAnalysis(id);
    void loadSelectedEvents(id);
    void loadSelectedSession(id);
    render();
  }
}

function renderInspector() {
  const node = selectedDetails?.id === selectedId ? selectedDetails : nodeMap.get(selectedId);
  if (!node) {
    if (selectedAnalysis || selectedAnalysisLoading) {
      details.innerHTML = renderSelectedAnalysis();
      return;
    }
    details.innerHTML = `<div class="empty">Select a node to inspect metadata and impact edges.</div>`;
    return;
  }
  const edges = selectedDetails?.id === selectedId && Array.isArray(selectedDetails.edges)
    ? selectedDetails.edges
    : graph.edges.filter((edge) => edge.source === node.id || edge.target === node.id);
  const edgeGroups = groupEdgesByKind(edges);
  const metadata = node.metadata && Object.keys(node.metadata).length
    ? `<div class="section-label">Metadata</div><pre class="metadata-json">${escapeHtml(JSON.stringify(node.metadata, null, 2))}</pre>`
    : "";
  const impactText = selectedImpactLoading && selectedId === node.id
    ? "Loading impact traversal..."
    : selectedImpact || "No impact traversal available yet.";
  const relatedEvents = renderSelectedEvents();
  const sessionDetails = renderSelectedSession();
  const analysisDetails = renderSelectedAnalysis();
  const relatedEventMetric = selectedEventLoading
    ? "Loading..."
    : `${selectedEventItems.length}`;
  details.innerHTML = `
    <div class="detail-title">
      <div class="detail-title-main">
        <strong>${escapeHtml(node.name)}</strong>
        <span class="kind-badge">${escapeHtml(node.kind)}</span>
      </div>
      <div class="detail-actions">
        <button class="detail-action" data-impact-target="${escapeHtml(node.id)}" ${selectedImpactLoading && selectedId === node.id ? "disabled" : ""}>
          ${selectedImpactLoading && selectedId === node.id ? "Loading..." : "Impact"}
        </button>
        <button class="detail-action" data-analysis-target="${escapeHtml(node.path || node.id)}" ${selectedAnalysisLoading && selectedId === node.id ? "disabled" : ""}>
          ${selectedAnalysisLoading && selectedId === node.id ? "Loading..." : "Analyze"}
        </button>
      </div>
    </div>
    <div class="meta-grid">
      <div class="meta-row"><span>ID</span><span>${escapeHtml(node.id)}</span></div>
      <div class="meta-row"><span>Path</span><span>${escapeHtml(node.path || "")}</span></div>
      <div class="meta-row"><span>Span</span><span>${escapeHtml(formatSpan(node.span_start, node.span_end))}</span></div>
      <div class="meta-row"><span>Hash</span><span>${escapeHtml(node.hash || "")}</span></div>
      <div class="meta-row"><span>Edges</span><span>${selectedDetailsLoading && selectedDetails?.id === selectedId ? "Loading..." : edges.length}</span></div>
      <div class="meta-row"><span>Mode</span><span>${focusMode ? "focus" : "overview"}</span></div>
    </div>
    ${metadata}
    ${sessionDetails}
    ${analysisDetails}
    <div class="section-label">Impact Edges</div>
    <div class="impact-list">
      ${renderEdgeGroups(edgeGroups)}
    </div>
    <div class="section-label">Full Impact</div>
    <pre class="impact-trace">${escapeHtml(impactText)}</pre>
    <div class="section-head">
      <span class="section-label">Related Events</span>
      <span class="section-metric">${escapeHtml(relatedEventMetric)}</span>
    </div>
    <div class="event-feed compact">
      ${relatedEvents}
    </div>
  `;
  wireImpactLinks(details);
  wireInspectorActions(details);
  wireEventLinks(details);
}

function sessionIdFromNodeId(nodeId) {
  const parts = String(nodeId || "").split(":");
  if (parts[0] !== "session" || parts.length < 3) {
    return null;
  }
  return parts.slice(2).join(":");
}

function renderSelectedSession() {
  if (!selectedId || !sessionIdFromNodeId(selectedId)) {
    return "";
  }
  if (selectedSessionLoading) {
    return '<div class="empty session-detail">Loading session messages...</div>';
  }
  if (!selectedSession?.session) {
    return '<div class="empty session-detail">No session details loaded.</div>';
  }
  const session = selectedSession.session;
  const messages = Array.isArray(selectedSession.messages) ? selectedSession.messages : [];
  return `
    <div class="session-detail">
      <div class="section-head">
        <span class="section-label">Session Messages</span>
        <span class="section-metric">${escapeHtml(String(messages.length))}</span>
      </div>
      <div class="meta-grid">
        <div class="meta-row"><span>Agent</span><span>${escapeHtml(session.agent || "")}</span></div>
        <div class="meta-row"><span>Started</span><span>${escapeHtml(formatTimestamp(session.started_at))}</span></div>
        <div class="meta-row"><span>Ended</span><span>${escapeHtml(formatTimestamp(session.ended_at))}</span></div>
        <div class="meta-row"><span>Tokens</span><span>${escapeHtml(String(session.token_count || 0))}</span></div>
      </div>
      <div class="session-messages">
        ${
          messages.length
            ? messages
                .map(
                  (message) => `
          <div class="session-message">
            <div class="session-message-head">
              <span class="session-role">${escapeHtml(message.role || "")}</span>
              <span>${escapeHtml(formatTimestamp(message.timestamp))}</span>
            </div>
            <div class="session-message-content">${escapeHtml(message.content || "")}</div>
          </div>
        `,
                )
                .join("")
            : '<div class="empty">No messages in session.</div>'
        }
      </div>
    </div>
  `;
}

function renderSelectedAnalysis() {
  if (!selectedAnalysis && !selectedAnalysisLoading) {
    return "";
  }
  if (selectedAnalysisLoading) {
    return '<div class="empty analysis-detail">Loading analysis...</div>';
  }
  if (!selectedAnalysis) {
    return '<div class="empty analysis-detail">No analysis loaded.</div>';
  }
  const nodes = Array.isArray(selectedAnalysis.graph_nodes) ? selectedAnalysis.graph_nodes : [];
  const edges = Array.isArray(selectedAnalysis.graph_edges) ? selectedAnalysis.graph_edges : [];
  const hits = Array.isArray(selectedAnalysis.search_hits) ? selectedAnalysis.search_hits : [];
  const cases = Array.isArray(selectedAnalysis.memory_cases) ? selectedAnalysis.memory_cases : [];
  const files = Array.isArray(selectedAnalysis.file_contexts) ? selectedAnalysis.file_contexts : [];
  return `
    <div class="analysis-detail">
      <div class="section-head">
        <span class="section-label">Analysis</span>
        <span class="section-metric">${escapeHtml(selectedAnalysis.target || selectedAnalysisTarget || "")}</span>
      </div>
      <button class="detail-action analysis-copy" data-copy-analysis-mermaid="1">Copy Mermaid</button>
      <div class="analysis-metrics">
        <span>${escapeHtml(String(nodes.length))} nodes</span>
        <span>${escapeHtml(String(edges.length))} edges</span>
        <span>${escapeHtml(String(hits.length))} hits</span>
        <span>${escapeHtml(String(cases.length))} memories</span>
        <span>${escapeHtml(String(files.length))} files</span>
      </div>
      ${selectedAnalysis.resolved_node ? `
        <div class="analysis-resolved">
          <span>${escapeHtml(selectedAnalysis.resolved_node.kind || "")}</span>
          <strong>${escapeHtml(selectedAnalysis.resolved_node.id || "")}</strong>
        </div>
      ` : '<div class="empty">No graph node resolved.</div>'}
      <div class="analysis-list">
        <div class="section-label">Graph Edges</div>
        ${edges.length ? edges.slice(0, 8).map((edge) => `
          <div class="analysis-row">${escapeHtml(edge.source || "")} <span>${escapeHtml(edge.kind || "")}</span> ${escapeHtml(edge.target || "")}</div>
        `).join("") : '<div class="empty">No graph edges in analysis.</div>'}
        <div class="section-label">Search Hits</div>
        ${hits.length ? hits.slice(0, 6).map((hit) => `
          <div class="analysis-row"><span>${escapeHtml(hit.kind || "")}</span> ${escapeHtml(hit.title || "")}</div>
        `).join("") : '<div class="empty">No search hits in analysis.</div>'}
        <div class="section-label">File Context</div>
        ${files.length ? files.slice(0, 4).map((file) => `
          <div class="analysis-file">
            <div class="analysis-file-head">
              <span>${escapeHtml(file.path || "")}</span>
              <small>${escapeHtml(file.hash || "")}</small>
            </div>
            <pre>${escapeHtml(file.snippet || "")}</pre>
          </div>
        `).join("") : '<div class="empty">No file context in analysis.</div>'}
        <div class="section-label">Memory Cases</div>
        ${cases.length ? cases.slice(0, 6).map((memoryCase) => `
          <div class="analysis-row"><span>${escapeHtml(memoryCase.id || "")}</span> ${escapeHtml(memoryCase.summary || memoryCase.name || "")}</div>
        `).join("") : '<div class="empty">No memory cases in analysis.</div>'}
      </div>
    </div>
  `;
}

function groupEdgesByKind(edges) {
  const groups = new Map();
  for (const edge of edges) {
    const kind = edge.kind || "edge";
    if (!groups.has(kind)) {
      groups.set(kind, []);
    }
    groups.get(kind).push(edge);
  }
  return Array.from(groups.entries()).map(([kind, items]) => ({ kind, items }));
}

function renderEdgeGroups(groups) {
  if (!groups.length) {
    return '<div class="empty">No impact edges.</div>';
  }
  return groups
    .map(
      (group) => `
        <div class="impact-group">
          <div class="impact-group-head">
            <span class="impact-kind">${escapeHtml(group.kind)}</span>
            <span class="impact-count">${group.items.length}</span>
          </div>
          <div class="impact-items">
            ${group.items
              .map(
                (edge) =>
                  `<div class="impact-item">
                    <button class="impact-link" data-node="${escapeHtml(edge.source)}">${escapeHtml(edge.source)}</button>
                    <span class="impact-arrow">→</span>
                    <button class="impact-link" data-node="${escapeHtml(edge.target)}">${escapeHtml(edge.target)}</button>
                  </div>`,
              )
              .join("")}
          </div>
        </div>
      `,
    )
    .join("");
}

function wireImpactLinks(root = details) {
  for (const button of root.querySelectorAll(".impact-link")) {
    button.addEventListener("click", () => {
      const nodeId = button.dataset.node;
      if (nodeId) {
        selectNode(nodeId);
      }
    });
  }
}

function wireInspectorActions(root = details) {
  for (const button of root.querySelectorAll(".detail-action")) {
    button.addEventListener("click", () => {
      const impactTarget = button.dataset.impactTarget;
      if (impactTarget) {
        void refreshSelectedImpact(impactTarget);
        return;
      }
      const analysisTarget = button.dataset.analysisTarget;
      if (analysisTarget) {
        void loadSelectedAnalysis(selectedId || analysisTarget, analysisTarget);
      }
    });
  }
  for (const button of root.querySelectorAll("[data-copy-analysis-mermaid]")) {
    button.addEventListener("click", async () => {
      if (selectedAnalysisMermaid) {
        await navigator.clipboard?.writeText(selectedAnalysisMermaid);
      }
    });
  }
}

async function updateSearchResults(query) {
  const trimmed = query.trim();
  const seq = ++searchSeq;
  if (!trimmed) {
    searchHits = [];
    updateSurfaceCounts([]);
    renderSearchResults();
    return;
  }
  try {
    const kind = searchKindsForSurface();
    const url = new URL(`${API_BASE}/search`);
    url.searchParams.set("q", trimmed);
    url.searchParams.set("limit", "10");
    if (kind) {
      url.searchParams.set("kind", kind);
    }
    const response = await fetch(url, {
      cache: "no-store",
    });
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== searchSeq) return;
    searchHits = payload.hits || [];
    updateSurfaceCounts(payload.surfaces || []);
  } catch (_) {
    if (seq !== searchSeq) return;
    searchHits = [];
    updateSurfaceCounts([]);
  }
  renderSearchResults();
}

async function loadEvents() {
  const seq = ++eventSeq;
  try {
    const response = await fetch(`${API_BASE}/events?limit=12`, {
      cache: "no-store",
    });
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== eventSeq) return;
    eventItems = payload.events || [];
    eventTotal = payload.total ?? eventItems.length;
  } catch (_) {
    if (seq !== eventSeq) return;
    eventItems = [];
    eventTotal = 0;
  }
  renderEventFeed();
}

async function loadSelectedEvents(id) {
  const seq = ++selectedEventSeq;
  try {
    const response = await fetch(
      `${API_BASE}/events?limit=12&node_id=${encodeURIComponent(id)}`,
      { cache: "no-store" },
    );
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== selectedEventSeq || selectedId !== id) return;
    selectedEventItems = payload.events || [];
  } catch (_) {
    if (seq !== selectedEventSeq || selectedId !== id) return;
    selectedEventItems = [];
  } finally {
    if (seq === selectedEventSeq && selectedId === id) {
      selectedEventLoading = false;
      renderInspector();
    }
  }
}

async function loadSelectedAnalysis(id, target = id) {
  const seq = ++selectedAnalysisSeq;
  selectedAnalysisTarget = target;
  selectedAnalysisLoading = true;
  renderInspector();
  try {
    const url = new URL(`${API_BASE}/analyze`);
    url.searchParams.set("target", target);
    url.searchParams.set("depth", `${focusDepth}`);
    url.searchParams.set("limit", "12");
    const response = await fetch(url, { cache: "no-store" });
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== selectedAnalysisSeq || selectedId !== id) return;
    selectedAnalysis = payload;
    selectedAnalysisMermaid = await fetchAnalysisMermaid(target);
  } catch (_) {
    if (seq === selectedAnalysisSeq && selectedId === id) {
      selectedAnalysis = null;
    }
  } finally {
    if (seq === selectedAnalysisSeq && selectedId === id) {
      selectedAnalysisLoading = false;
      renderInspector();
    }
  }
}

async function runTargetAnalysis(target) {
  const trimmed = target.trim();
  if (!trimmed) return;
  const seq = ++selectedAnalysisSeq;
  selectedId = null;
  selectedDetails = null;
  selectedDetailsLoading = false;
  selectedImpact = "";
  selectedImpactLoading = false;
  selectedEventItems = [];
  selectedEventLoading = false;
  selectedSnapshot = null;
  selectedSnapshotLoading = false;
  selectedSession = null;
  selectedSessionLoading = false;
  selectedAnalysis = null;
  selectedAnalysisTarget = trimmed;
  selectedAnalysisLoading = true;
  selectedAnalysisMermaid = "";
  persistDashboardState({ node: null });
  render();
  try {
    const url = new URL(`${API_BASE}/analyze`);
    url.searchParams.set("target", trimmed);
    url.searchParams.set("depth", `${focusDepth}`);
    url.searchParams.set("limit", "12");
    const response = await fetch(url, { cache: "no-store" });
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== selectedAnalysisSeq || selectedId !== null) return;
    selectedAnalysis = payload;
    selectedAnalysisMermaid = await fetchAnalysisMermaid(trimmed);
  } catch (_) {
    if (seq === selectedAnalysisSeq && selectedId === null) {
      selectedAnalysis = null;
    }
  } finally {
    if (seq === selectedAnalysisSeq && selectedId === null) {
      selectedAnalysisLoading = false;
      renderInspector();
    }
  }
}

async function fetchAnalysisMermaid(target) {
  const url = new URL(`${API_BASE}/analyze`);
  url.searchParams.set("target", target);
  url.searchParams.set("depth", `${focusDepth}`);
  url.searchParams.set("limit", "12");
  url.searchParams.set("format", "mermaid");
  const response = await fetch(url, { cache: "no-store" });
  if (!response.ok) return "";
  return response.text();
}

async function loadSelectedSession(nodeId) {
  const sessionId = sessionIdFromNodeId(nodeId);
  const seq = ++selectedSessionSeq;
  if (!sessionId) {
    selectedSession = null;
    selectedSessionLoading = false;
    return;
  }
  selectedSessionLoading = true;
  renderInspector();
  try {
    const response = await fetch(`${API_BASE}/session/${encodeURIComponent(sessionId)}?limit=100`, {
      cache: "no-store",
    });
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== selectedSessionSeq || selectedId !== nodeId) return;
    selectedSession = payload;
  } catch (_) {
    if (seq === selectedSessionSeq && selectedId === nodeId) {
      selectedSession = null;
    }
  } finally {
    if (seq === selectedSessionSeq && selectedId === nodeId) {
      selectedSessionLoading = false;
      renderInspector();
    }
  }
}

async function loadStatus() {
  try {
    const response = await fetch(`${API_BASE}/status`, { cache: "no-store" });
    if (!response.ok) return;
    const payload = await response.json();
    if (typeof payload.events === "number") {
      eventTotal = payload.events;
      renderEventFeed();
    }
    if (typeof payload.snapshots === "number") {
      snapshotTotal = payload.snapshots;
      renderSnapshotFeed();
    }
  } catch (_) {
    // ignore
  }
}

async function loadSnapshots() {
  const seq = ++snapshotSeq;
  try {
    const response = await fetch(`${API_BASE}/snapshots?limit=8`, { cache: "no-store" });
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== snapshotSeq) return;
    snapshotItems = payload.snapshots || [];
    snapshotTotal = payload.total ?? snapshotItems.length;
  } catch (_) {
    if (seq !== snapshotSeq) return;
    snapshotItems = [];
    snapshotTotal = 0;
  }
  renderSnapshotFeed();
}

async function loadAdapters() {
  const seq = ++adapterSeq;
  try {
    const response = await fetch(`${API_BASE}/adapters?limit=8`, { cache: "no-store" });
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== adapterSeq) return;
    adapterItems = payload.adapters || [];
    adapterTotal = payload.total ?? adapterItems.length;
  } catch (_) {
    if (seq !== adapterSeq) return;
    adapterItems = [];
    adapterTotal = 0;
  }
  renderAdapterFeed();
}

async function loadMemoryCases() {
  const seq = ++memorySeq;
  try {
    const response = await fetch(`${API_BASE}/memory-cases?limit=8`, { cache: "no-store" });
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== memorySeq) return;
    memoryItems = payload.memory_cases || [];
    memoryTotal = payload.total ?? memoryItems.length;
  } catch (_) {
    if (seq !== memorySeq) return;
    memoryItems = [];
    memoryTotal = 0;
  }
  renderMemoryFeed();
}

async function loadSessions() {
  const seq = ++sessionSeq;
  try {
    const response = await fetch(`${API_BASE}/sessions?limit=8`, { cache: "no-store" });
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== sessionSeq) return;
    sessionItems = payload.sessions || [];
    sessionTotal = payload.total ?? sessionItems.length;
  } catch (_) {
    if (seq !== sessionSeq) return;
    sessionItems = [];
    sessionTotal = 0;
  }
  renderSessionFeed();
}

async function loadEmbeddings() {
  const seq = ++embeddingSeq;
  try {
    const response = await fetch(`${API_BASE}/embeddings?limit=8`, { cache: "no-store" });
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== embeddingSeq) return;
    embeddingItems = payload.embeddings || [];
    embeddingTotal = payload.total ?? embeddingItems.length;
    embeddingPath = payload.path || "";
  } catch (_) {
    if (seq !== embeddingSeq) return;
    embeddingItems = [];
    embeddingTotal = 0;
    embeddingPath = "";
  }
  renderEmbeddingFeed();
}

async function loadEmbeddingSearch(query) {
  embeddingSearchQuery = query.trim();
  if (!embeddingSearchQuery) {
    embeddingSearchHits = [];
    renderEmbeddingFeed();
    return;
  }
  const seq = ++embeddingSearchSeq;
  try {
    const response = await fetch(`${API_BASE}/embeddings/search?q=${encodeURIComponent(embeddingSearchQuery)}&limit=8`, { cache: "no-store" });
    if (!response.ok) return;
    const payload = await response.json();
    if (seq !== embeddingSearchSeq) return;
    embeddingSearchHits = payload.hits || [];
  } catch (_) {
    if (seq !== embeddingSearchSeq) return;
    embeddingSearchHits = [];
  }
  renderEmbeddingFeed();
}

function renderEventFeed() {
  if (!eventFeed) return;
  if (eventSummary) {
    eventSummary.textContent = `${eventTotal}`;
  }
  if (!eventItems.length) {
    eventFeed.innerHTML = '<div class="empty">No recent events.</div>';
    return;
  }
  eventFeed.innerHTML = renderEventCards(eventItems, false);
  wireEventLinks(eventFeed);
}

function renderSnapshotFeed() {
  if (!snapshotFeed) return;
  if (snapshotSummary) {
    snapshotSummary.textContent = `${snapshotTotal}`;
  }
  if (snapshotCount) {
    snapshotCount.textContent = `${snapshotTotal}`;
  }
  if (!snapshotItems.length) {
    snapshotFeed.innerHTML = '<div class="empty">No recent snapshots.</div>';
    return;
  }
  snapshotFeed.innerHTML = `
    ${renderSnapshotCards(snapshotItems)}
    ${renderSelectedSnapshot()}
  `;
  wireSnapshotLinks(snapshotFeed);
}

function renderAdapterFeed() {
  if (!adapterFeed) return;
  if (adapterSummary) {
    adapterSummary.textContent = `${adapterTotal}`;
  }
  if (!adapterItems.length) {
    adapterFeed.innerHTML = '<div class="empty">No adapters registered.</div>';
    return;
  }
  adapterFeed.innerHTML = renderAdapterCards(adapterItems);
  wireAdapterLinks(adapterFeed);
}

function renderMemoryFeed() {
  if (!memoryFeed) return;
  if (memorySummary) {
    memorySummary.textContent = `${memoryTotal}`;
  }
  if (!memoryItems.length) {
    memoryFeed.innerHTML = '<div class="empty">No memory cases pinned.</div>';
    return;
  }
  memoryFeed.innerHTML = renderMemoryCards(memoryItems);
  wireMemoryLinks(memoryFeed);
}

function renderSessionFeed() {
  if (!sessionFeed) return;
  if (sessionSummary) {
    sessionSummary.textContent = `${sessionTotal}`;
  }
  if (!sessionItems.length) {
    sessionFeed.innerHTML = '<div class="empty">No sessions imported.</div>';
    return;
  }
  sessionFeed.innerHTML = renderSessionCards(sessionItems);
  wireSessionLinks(sessionFeed);
}

function renderEmbeddingFeed() {
  if (!embeddingFeed) return;
  if (embeddingSummary) {
    embeddingSummary.textContent = `${embeddingTotal}`;
  }
  if (embeddingSearchQuery) {
    if (!embeddingSearchHits.length) {
      embeddingFeed.innerHTML = '<div class="empty">No embedding hits.</div>';
      return;
    }
    embeddingFeed.innerHTML = renderEmbeddingSearchCards(embeddingSearchHits, embeddingSearchQuery);
    return;
  }
  if (!embeddingItems.length) {
    embeddingFeed.innerHTML = '<div class="empty">No embeddings built.</div>';
    return;
  }
  embeddingFeed.innerHTML = renderEmbeddingCards(embeddingItems, embeddingPath);
}

function renderSelectedEvents() {
  if (selectedEventLoading) {
    return '<div class="empty">Loading related events...</div>';
  }
  if (!selectedEventItems.length) {
    return '<div class="empty">No related events.</div>';
  }
  return renderEventCards(selectedEventItems, true);
}

function renderAdapterCards(items) {
  return items
    .map((adapter) => {
      const nodeId = `adapter:${adapter.id}`;
      const active = selectedId === nodeId ? " active" : "";
      const state = adapter.enabled ? "enabled" : "disabled";
      return `
        <div class="adapter-card${active}" data-node-id="${escapeHtml(nodeId)}">
          <div class="adapter-head">
            <span class="adapter-name">${escapeHtml(adapter.name || adapter.id || "")}</span>
            <span class="adapter-state">${escapeHtml(state)}</span>
          </div>
          <div class="adapter-meta">
            <span>${escapeHtml(adapter.agent || "")}</span>
            <span>${escapeHtml(adapter.command || "")}</span>
          </div>
          <div class="adapter-path">${escapeHtml(adapter.session_dir || "")}</div>
        </div>
      `;
    })
    .join("");
}

function renderMemoryCards(items) {
  return items
    .map((memoryCase) => {
      const nodeId = memoryCase.id || "";
      const active = selectedId === nodeId ? " active" : "";
      return `
        <div class="memory-card${active}" data-node-id="${escapeHtml(nodeId)}">
          <div class="memory-head">
            <span class="memory-name">${escapeHtml(memoryCase.name || nodeId)}</span>
            <span class="memory-target">${escapeHtml(memoryCase.target || "")}</span>
          </div>
          <div class="memory-summary">${escapeHtml(memoryCase.summary || "")}</div>
          <div class="memory-meta">
            <span>${escapeHtml(formatTimestamp(memoryCase.updated_at))}</span>
            <span>${escapeHtml(nodeId)}</span>
          </div>
        </div>
      `;
    })
    .join("");
}

function renderSessionCards(items) {
  return items
    .map((session) => {
      const nodeId = `session:${session.agent}:${session.id}`;
      const active = selectedId === nodeId ? " active" : "";
      return `
        <div class="session-card${active}" data-node-id="${escapeHtml(nodeId)}">
          <div class="session-head">
            <span class="session-name">${escapeHtml(session.id || "")}</span>
            <span class="session-agent">${escapeHtml(session.agent || "")}</span>
          </div>
          <div class="session-meta">
            <span>${escapeHtml(String(session.message_count || 0))} messages</span>
            <span>${escapeHtml(formatTimestamp(session.started_at))}</span>
          </div>
        </div>
      `;
    })
    .join("");
}

function renderEmbeddingCards(items, path) {
  const store = path
    ? `<div class="embedding-store">${escapeHtml(path)}</div>`
    : "";
  return `
    ${store}
    ${items
      .map((record) => {
        const metadata = record.metadata && typeof record.metadata === "object"
          ? JSON.stringify(record.metadata)
          : "";
        return `
          <div class="embedding-card">
            <div class="embedding-head">
              <span class="embedding-type">${escapeHtml(record.chunk_type || "")}</span>
              <span class="embedding-offset">${escapeHtml(String(record.embedding_offset ?? ""))}</span>
            </div>
            <div class="embedding-chunk">${escapeHtml(record.chunk_id || "")}</div>
            <div class="embedding-meta">${escapeHtml(metadata)}</div>
          </div>
        `;
      })
      .join("")}
  `;
}

function renderEmbeddingSearchCards(items, query) {
  return `
    <div class="embedding-store">Search: ${escapeHtml(query)}</div>
    ${items
      .map((hit) => {
        const metadata = hit.metadata && typeof hit.metadata === "object"
          ? JSON.stringify(hit.metadata)
          : "";
        return `
          <div class="embedding-card">
            <div class="embedding-head">
              <span class="embedding-type">${escapeHtml(hit.chunk_type || "")}</span>
              <span class="embedding-offset">${escapeHtml(Number(hit.score || 0).toFixed(4))}</span>
            </div>
            <div class="embedding-chunk">${escapeHtml(hit.snippet || hit.chunk_id || "")}</div>
            <div class="embedding-meta">${escapeHtml(metadata)}</div>
          </div>
        `;
      })
      .join("")}
  `;
}

function renderSnapshotCards(items) {
  return items
    .map((snapshot) => {
      const active = selectedSnapshot?.snapshot?.hash === snapshot.hash ? " active" : "";
      return `
        <div class="snapshot-card${active}" data-snapshot-hash="${escapeHtml(snapshot.hash)}">
          <div class="snapshot-head">
            <span class="snapshot-hash">${escapeHtml(snapshot.hash.slice(0, 12))}</span>
            <span class="snapshot-count">${escapeHtml(String(snapshot.file_count || 0))} files</span>
          </div>
          <div class="snapshot-message">${escapeHtml(snapshot.message || "")}</div>
          <div class="snapshot-meta">
            <span>${escapeHtml(formatTimestamp(snapshot.timestamp))}</span>
            <span>${escapeHtml(formatBytes(snapshot.total_size || 0))}</span>
          </div>
        </div>
      `;
    })
    .join("");
}

function renderSelectedSnapshot() {
  if (selectedSnapshotLoading) {
    return '<div class="empty snapshot-detail">Loading snapshot details...</div>';
  }
  const details = selectedSnapshot?.snapshot;
  if (!details) {
    return '<div class="empty snapshot-detail">Select a snapshot to inspect its files.</div>';
  }
  const files = Array.isArray(selectedSnapshot.files) ? selectedSnapshot.files : [];
  return `
    <div class="snapshot-detail">
      <div class="section-label">Snapshot Details</div>
      <div class="meta-grid">
        <div class="meta-row"><span>Hash</span><span>${escapeHtml(details.hash)}</span></div>
        <div class="meta-row"><span>Message</span><span>${escapeHtml(details.message || "")}</span></div>
        <div class="meta-row"><span>Files</span><span>${escapeHtml(String(details.file_count || 0))}</span></div>
        <div class="meta-row"><span>Size</span><span>${escapeHtml(formatBytes(details.total_size || 0))}</span></div>
        <div class="meta-row"><span>Timestamp</span><span>${escapeHtml(formatTimestamp(details.timestamp))}</span></div>
      </div>
      <div class="section-label">Files</div>
      <div class="snapshot-files">
        ${
          files.length
            ? files
                .map(
                  (file) => `
          <div class="snapshot-file">
            <div class="snapshot-file-path">${escapeHtml(file.path || "")}</div>
            <div class="snapshot-file-meta">
              <span>${escapeHtml(file.object_hash || "")}</span>
              <span>${escapeHtml(formatBytes(file.size || 0))}</span>
            </div>
          </div>
        `,
                )
                .join("")
            : '<div class="empty">No files in snapshot.</div>'
        }
      </div>
    </div>
  `;
}

function wireAdapterLinks(root = adapterFeed) {
  for (const card of root.querySelectorAll(".adapter-card")) {
    card.addEventListener("click", () => {
      const nodeId = card.dataset.nodeId;
      if (nodeId) {
        void focusNode(nodeId);
      }
    });
  }
}

function wireMemoryLinks(root = memoryFeed) {
  for (const card of root.querySelectorAll(".memory-card")) {
    card.addEventListener("click", () => {
      const nodeId = card.dataset.nodeId;
      if (nodeId) {
        void focusNode(nodeId);
      }
    });
  }
}

function wireSessionLinks(root = sessionFeed) {
  for (const card of root.querySelectorAll(".session-card")) {
    card.addEventListener("click", () => {
      const nodeId = card.dataset.nodeId;
      if (nodeId) {
        void focusNode(nodeId);
      }
    });
  }
}

function wireSnapshotLinks(root = snapshotFeed) {
  for (const card of root.querySelectorAll(".snapshot-card")) {
    card.addEventListener("click", () => {
      const hash = card.dataset.snapshotHash;
      if (!hash) return;
      selectedSnapshot = { snapshot: { hash }, files: [] };
      selectedSnapshotLoading = true;
      renderSnapshotFeed();
      void loadSnapshotDetails(hash);
    });
  }
}

async function loadSnapshotDetails(hash) {
  try {
    const response = await fetch(`${API_BASE}/snapshot/${encodeURIComponent(hash)}`, {
      cache: "no-store",
    });
    if (!response.ok) return;
    const payload = await response.json();
    if (selectedSnapshot?.snapshot?.hash !== hash && selectedSnapshot?.hash !== hash) return;
    selectedSnapshot = payload;
  } catch (_) {
    if (selectedSnapshot?.snapshot?.hash === hash || selectedSnapshot?.hash === hash) {
      selectedSnapshot = null;
    }
  } finally {
    selectedSnapshotLoading = false;
    renderSnapshotFeed();
  }
}

function renderEventCards(items, compact = false) {
  return items
    .map((event) => {
      const payload = event.event_data && typeof event.event_data === "object"
        ? JSON.stringify(event.event_data, null, 2)
        : String(event.event_data || "");
      const clickable = event.node_id ? "clickable" : "";
      const compactClass = compact ? " compact" : "";
      return `
        <div class="event-card ${clickable}${compactClass}" data-node-id="${escapeHtml(event.node_id || "")}">
          <div class="event-head">
            <span class="event-kind">${escapeHtml(event.event_type || "event")}</span>
            <span class="event-status">${escapeHtml(event.status || "pending")}</span>
          </div>
          <div class="event-meta">
            <span>${escapeHtml(event.source || "")}</span>
            <span>${escapeHtml(formatTimestamp(event.timestamp))}</span>
          </div>
          <pre class="event-payload">${escapeHtml(payload)}</pre>
        </div>
      `;
    })
    .join("");
}

function wireEventLinks(root = eventFeed) {
  for (const card of root.querySelectorAll(".event-card.clickable")) {
    card.addEventListener("click", () => {
      const nodeId = card.dataset.nodeId;
      if (nodeId) {
        void focusNode(nodeId);
      }
    });
  }
}

function renderSearchResults() {
  if (!searchResults) return;
  const query = searchInput.value.trim();
  if (!query) {
    searchResults.innerHTML = '<div class="empty">Type to search the local index.</div>';
    return;
  }
  if (!searchHits.length) {
    searchResults.innerHTML = '<div class="empty">No local search hits.</div>';
    return;
  }
  searchResults.innerHTML = searchHits
    .map((hit, index) => {
      const clickable = hit.node_id || hit.snapshot_hash ? "clickable" : "";
      const kindClass = searchHitClass(hit.kind);
      const stateLabel = hit.snapshot_hash ? "Open" : hit.node_id ? "Focus" : "Text";
      const stateClass = hit.snapshot_hash || hit.node_id ? "focusable" : "static";
      return `
        <div class="search-hit ${clickable}" data-index="${index}">
          <div class="hit-head">
            <div class="hit-meta">
              <span class="hit-kind kind-${kindClass}">${escapeHtml(hit.kind)}</span>
              <span class="hit-state ${stateClass}">${stateLabel}</span>
            </div>
            <div class="hit-title">${escapeHtml(hit.title || "")}</div>
          </div>
          ${hit.path ? `<div class="hit-path">${escapeHtml(hit.path)}</div>` : ""}
          ${hit.snippet ? `<div class="hit-snippet">${escapeHtml(hit.snippet)}</div>` : ""}
        </div>
      `;
    })
    .join("");

  for (const element of searchResults.querySelectorAll(".search-hit.clickable")) {
    element.addEventListener("click", () => {
      const hit = searchHits[Number(element.dataset.index)];
      if (hit?.snapshot_hash) {
        selectedSnapshot = { snapshot: { hash: hit.snapshot_hash }, files: [] };
        selectedSnapshotLoading = true;
        renderSnapshotFeed();
        if (hit.node_id) {
          void focusNode(hit.node_id);
        }
        void loadSnapshotDetails(hit.snapshot_hash);
      } else if (hit?.node_id) {
        void focusNode(hit.node_id);
      }
    });
  }
}

function navigatorTraversalNodes() {
  if (!selectedId) return [];
  const edges = (selectedDetails?.id === selectedId && Array.isArray(selectedDetails.edges)
    ? selectedDetails.edges
    : graph.edges.filter((edge) => edge.source === selectedId || edge.target === selectedId)
  ).filter((edge) => enabledEdgeKinds.has(edge.kind));
  const related = new Map();
  for (const edge of edges) {
    const otherId = edge.source === selectedId ? edge.target : edge.source;
    if (!otherId || related.has(otherId)) continue;
    const node = (selectedDetails?.id === selectedId && Array.isArray(selectedDetails.nodes)
      ? selectedDetails.nodes.find((entry) => entry.id === otherId)
      : null) || nodeMap.get(otherId) || { id: otherId, kind: "Node", name: otherId.split(":").pop(), path: otherId };
    const direction = edge.source === selectedId ? "out" : "in";
    related.set(otherId, {
      id: otherId,
      kind: node.kind || "Node",
      name: node.name || otherId,
      path: node.path || otherId,
      relation: `${direction} ${edge.kind}`,
    });
  }
  return [...related.values()].slice(0, 120);
}

function renderNavigator() {
  if (!navigatorList) return;
  const traversal = navigatorTraversalNodes();
  const nodes = nodesForSurface().slice(0, 120);
  if (navigatorTraversalCount) {
    navigatorTraversalCount.textContent = `${traversal.length}`;
  }
  if (navigatorCount) {
    navigatorCount.textContent = `${nodes.length}`;
  }
  if (navigatorTraversal) {
    if (!selectedId) {
      navigatorTraversal.innerHTML = '<div class="empty">Select a node to browse its neighborhood.</div>';
    } else if (!traversal.length) {
      navigatorTraversal.innerHTML = '<div class="empty">No connected nodes at the current depth.</div>';
    } else {
      navigatorTraversal.innerHTML = traversal
        .map((item) => {
          const active = item.id === selectedId ? " active" : "";
          return `
            <article class="navigator-card${active}" data-node-id="${escapeHtml(item.id)}">
              <div class="navigator-head">
                <span class="kind-badge">${escapeHtml(item.kind)}</span>
                <span class="navigator-id">${escapeHtml(item.relation)}</span>
              </div>
              <div class="navigator-title">${escapeHtml(item.name)}</div>
              <div class="navigator-path">${escapeHtml(item.path || item.id)}</div>
              <div class="navigator-actions">
                <button class="navigator-action" data-action="inspect">Inspect</button>
                <button class="navigator-action" data-action="focus">Focus</button>
              </div>
            </article>
          `;
        })
        .join("");
    }
    for (const card of navigatorTraversal.querySelectorAll(".navigator-card")) {
      card.addEventListener("click", (event) => {
        const target = event.target;
        const nodeId = card.dataset.nodeId;
        if (!nodeId) return;
        if (target instanceof HTMLElement && target.dataset.action === "focus") {
          void focusNode(nodeId);
          return;
        }
        if (target instanceof HTMLElement && target.dataset.action === "inspect") {
          selectNode(nodeId);
          return;
        }
        selectNode(nodeId);
      });
      card.addEventListener("dblclick", () => {
        const nodeId = card.dataset.nodeId;
        if (nodeId) {
          void focusNode(nodeId);
        }
      });
    }
  }
  if (!nodes.length) {
    navigatorList.innerHTML = `<div class="empty">No visible ${activeSurface} nodes. Adjust filters or search terms.</div>`;
    return;
  }
  navigatorList.innerHTML = nodes
    .map((node) => {
      const active = node.id === selectedId ? " active" : "";
      return `
        <article class="navigator-card${active}" data-node-id="${escapeHtml(node.id)}">
          <div class="navigator-head">
            <span class="kind-badge">${escapeHtml(node.kind)}</span>
            <span class="navigator-id">${escapeHtml(node.id)}</span>
          </div>
          <div class="navigator-title">${escapeHtml(node.name)}</div>
          <div class="navigator-path">${escapeHtml(node.path || node.id)}</div>
          <div class="navigator-actions">
            <button class="navigator-action" data-action="inspect">Inspect</button>
            <button class="navigator-action" data-action="focus">Focus</button>
          </div>
        </article>
      `;
    })
    .join("");

  for (const card of navigatorList.querySelectorAll(".navigator-card")) {
    card.addEventListener("click", (event) => {
      const target = event.target;
      const nodeId = card.dataset.nodeId;
      if (!nodeId) return;
      if (target instanceof HTMLElement && target.dataset.action === "focus") {
        void focusNode(nodeId);
        return;
      }
      if (target instanceof HTMLElement && target.dataset.action === "inspect") {
        selectNode(nodeId);
        return;
      }
      selectNode(nodeId);
    });
    card.addEventListener("dblclick", () => {
      const nodeId = card.dataset.nodeId;
      if (nodeId) {
        void focusNode(nodeId);
      }
    });
  }
}

async function refreshSelectedDetails(id) {
  const seq = ++selectedDetailsSeq;
  const depth = focusDepth;
  selectedDetailsLoading = true;
  renderInspector();
  try {
    const response = await fetch(`${API_BASE}/graph/${encodeURIComponent(id)}?depth=${depth}`, {
      cache: "no-store",
    });
    if (!response.ok) return;
    const subset = await response.json();
    if (seq !== selectedDetailsSeq || selectedId !== id || depth !== focusDepth) return;
    selectedDetails = subset.focus
      ? { ...subset.focus, nodes: subset.nodes || [], edges: subset.edges || [] }
      : null;
  } catch (_) {
    if (seq === selectedDetailsSeq && selectedId === id) {
      selectedDetails = null;
    }
  } finally {
    if (seq === selectedDetailsSeq && selectedId === id) {
      selectedDetailsLoading = false;
      renderInspector();
    }
  }
}

async function refreshSelectedImpact(id) {
  const seq = ++selectedImpactSeq;
  const depth = focusDepth;
  selectedImpactLoading = true;
  renderInspector();
  try {
    const response = await fetch(
      `${API_BASE}/impact?target=${encodeURIComponent(id)}&depth=${depth}&limit=25`,
      { cache: "no-store" },
    );
    const text = await response.text();
    if (seq !== selectedImpactSeq || selectedId !== id || depth !== focusDepth) return;
    selectedImpact = response.ok ? text : `Impact request failed: ${text || response.statusText}`;
  } catch (_) {
    if (seq === selectedImpactSeq && selectedId === id) {
      selectedImpact = "Impact request unavailable.";
    }
  } finally {
    if (seq === selectedImpactSeq && selectedId === id) {
      selectedImpactLoading = false;
      renderInspector();
    }
  }
}

function curve(x1, y1, x2, y2) {
  const midY = (y1 + y2) / 2;
  return `M${x1},${y1 + 30} C${x1},${midY} ${x2},${midY} ${x2},${y2 - 30}`;
}

function svg(tag, attrs = {}) {
  const element = document.createElementNS("http://www.w3.org/2000/svg", tag);
  for (const [key, value] of Object.entries(attrs)) {
    element.setAttribute(key, value);
  }
  return element;
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#039;" })[ch]);
}

function searchHitClass(value) {
  return String(value || "other")
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "") || "other";
}

function formatSpan(start, end) {
  if (start == null && end == null) return "";
  return `${start ?? "?"} - ${end ?? "?"}`;
}

function formatTimestamp(timestamp) {
  if (timestamp == null) return "";
  const value = Number(timestamp) * 1000;
  if (!Number.isFinite(value)) return String(timestamp);
  try {
    return new Date(value).toISOString();
  } catch (_) {
    return String(timestamp);
  }
}

function formatBytes(bytes) {
  const value = Number(bytes);
  if (!Number.isFinite(value)) return String(bytes);
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
}

kindFilters.addEventListener("change", (event) => {
  const kind = event.target.dataset.kind;
  if (!kind) return;
  if (event.target.checked) enabledKinds.add(kind);
  else enabledKinds.delete(kind);
  render();
});

edgeLegend.addEventListener("keydown", (event) => {
  if (event.key !== "Enter" && event.key !== " ") return;
  const target = event.target;
  if (!(target instanceof HTMLElement)) return;
  const kind = target.dataset.kind;
  if (!kind) return;
  event.preventDefault();
  if (enabledEdgeKinds.has(kind)) {
    enabledEdgeKinds.delete(kind);
  } else {
    enabledEdgeKinds.add(kind);
  }
  hydrateFilters();
  render();
});

searchInput.addEventListener("input", render);
searchInput.addEventListener("input", () => {
  void updateSearchResults(searchInput.value);
});
filterInput.addEventListener("input", render);
focusDepthInput.addEventListener("change", (event) => {
  const target = event.currentTarget;
  if (!(target instanceof HTMLSelectElement)) return;
  setFocusDepth(target.value);
});
for (const button of document.querySelectorAll(".tabs button[data-tab]")) {
  button.addEventListener("click", () => {
    const tab = button.dataset.tab;
    if (tab) setActiveTab(tab);
  });
}
for (const button of document.querySelectorAll(".rail-button[data-surface]")) {
  button.addEventListener("click", () => {
    const surface = button.dataset.surface;
    if (surface) setActiveSurface(surface);
  });
}
resetFilter.addEventListener("click", async () => {
  searchInput.value = "";
  if (analyzeTargetInput) analyzeTargetInput.value = "";
  filterInput.value = "";
  searchHits = [];
  selectedImpact = "";
  selectedImpactLoading = false;
  selectedAnalysis = null;
  selectedAnalysisLoading = false;
  selectedAnalysisTarget = "";
  selectedAnalysisMermaid = "";
  selectedEventItems = [];
  selectedEventLoading = false;
  selectedSnapshot = null;
  selectedSnapshotLoading = false;
  renderSearchResults();
  enabledKinds = new Set(graph.nodes.map((node) => node.kind));
  focusMode = false;
  selectedId = null;
  persistDashboardState({ node: null });
  await loadGraph();
  for (const input of kindFilters.querySelectorAll("input")) input.checked = true;
  render();
});

analyzeTargetButton?.addEventListener("click", () => {
  void runTargetAnalysis(analyzeTargetInput?.value || "");
});

analyzeTargetInput?.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    void runTargetAnalysis(analyzeTargetInput.value);
  } else if (event.key === "Escape") {
    analyzeTargetInput.value = "";
  }
});

fitButton.addEventListener("click", () => {
  view = { x: 0, y: 0, scale: 1 };
  render();
});

exportButton.addEventListener("click", async () => {
  const target = focusMode && selectedId ? selectedId : null;
  const url = target
    ? `${API_BASE}/graph/${encodeURIComponent(target)}?format=mermaid&depth=${focusDepth}`
    : null;
  if (url) {
    const response = await fetch(url);
    if (response.ok) {
      const text = await response.text();
      await navigator.clipboard?.writeText(text);
      return;
    }
  }
  const text = ["flowchart TD", ...graph.edges.map((edge) => `  ${safeId(edge.source)} -->|${edge.kind}| ${safeId(edge.target)}`)].join("\n");
  await navigator.clipboard?.writeText(text);
});

embeddingSearchButton?.addEventListener("click", () => {
  void loadEmbeddingSearch(embeddingSearchInput?.value || "");
});

embeddingSearchInput?.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    void loadEmbeddingSearch(embeddingSearchInput.value);
  } else if (event.key === "Escape") {
    embeddingSearchInput.value = "";
    void loadEmbeddingSearch("");
  }
});

canvas.addEventListener("pointerdown", (event) => {
  drag = { x: event.clientX, y: event.clientY, startX: view.x, startY: view.y };
});

canvas.addEventListener("pointermove", (event) => {
  if (!drag) return;
  view.x = drag.startX - (event.clientX - drag.x);
  view.y = drag.startY - (event.clientY - drag.y);
  render();
});

window.addEventListener("pointerup", () => {
  drag = null;
});

canvas.addEventListener(
  "wheel",
  (event) => {
    event.preventDefault();
    view.scale = Math.min(1.8, Math.max(0.55, view.scale + (event.deltaY > 0 ? -0.08 : 0.08)));
    render();
  },
  { passive: false },
);

function safeId(id) {
  return `n_${id.replace(/[^a-zA-Z0-9]/g, "_")}`;
}

window.addEventListener("resize", render);
const initialGraphLoad = loadGraph();
renderEventFeed();
renderSnapshotFeed();
renderAdapterFeed();
renderMemoryFeed();
renderSessionFeed();
renderEmbeddingFeed();
refreshTimer = setInterval(() => {
  if (!drag) {
    loadGraph({ preserveSelection: true });
  }
  void loadStatus();
  void loadEvents();
  void loadSnapshots();
  void loadAdapters();
  void loadMemoryCases();
  void loadSessions();
  void loadEmbeddings();
}, REFRESH_MS);
initialGraphLoad.then(() => {
  if (initialSelectedId && nodeMap.has(initialSelectedId)) {
    selectNode(initialSelectedId);
  }
  initialSelectedId = null;
});
