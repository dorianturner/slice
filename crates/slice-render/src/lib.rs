//! Self-contained HTML report generation.
//!
//! The report deliberately contains its data, CSS, and JavaScript so a profile
//! can be opened directly from `file://` without a server or network access.

use serde::Serialize;
use slice_core::{
    Metric, PercentileRange, Profile, ProfileValidationError, Query, QueryError, TimeRange,
    execute_query,
};

#[derive(Clone, Debug, Serialize)]
struct InitialQuery {
    function_id: u32,
    threads: Option<Vec<u32>>,
    time: Option<TimeRange>,
    percentile: PercentileRange,
    metric: Metric,
}

impl From<&Query> for InitialQuery {
    fn from(query: &Query) -> Self {
        Self {
            function_id: query.function_id,
            threads: query
                .threads
                .as_ref()
                .map(|threads| threads.iter().copied().collect()),
            time: query.time,
            percentile: query.percentile,
            metric: query.metric,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("invalid profile: {0}")]
    InvalidProfile(#[from] ProfileValidationError),
    #[error("invalid viewer query: {0}")]
    InvalidQuery(#[from] QueryError),
    #[error("viewer serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Return a complete offline HTML viewer.
pub fn render_html(profile: &Profile, query: &Query) -> Result<String, RenderError> {
    profile.validate()?;
    execute_query(profile, query)?;
    let profile_json = script_safe_json(profile)?;
    let initial_json = script_safe_json(&InitialQuery::from(query))?;
    Ok(format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Slice profile</title>
  <style>{}</style>
</head>
<body>
<main>
  <header class="topbar"><h1>Slice</h1></header>
  <noscript><div class="viewer-error" role="alert">This offline report needs JavaScript enabled to render its interactive controls.</div></noscript>
  <div id="viewer-error" class="viewer-error" role="alert" hidden></div>
  <section class="population" aria-label="Population">
    <div><p class="label">Population</p><h2 id="population-name"></h2><p id="population-detail" class="muted"></p></div>
    <div class="capture-meta"><span id="capture-command"></span><span id="capture-range"></span></div>
  </section>
  <section class="timeline-section" aria-label="Capture timeline">
    <div class="section-heading"><div><p class="label">Capture timeline</p><h2>Thread activity</h2></div><div class="timeline-actions"><details id="thread-picker" class="thread-picker"><summary id="thread-summary">All observed threads</summary><div id="threads" class="thread-list"></div></details><div id="time-label" class="range-editor" aria-label="Selected time window"><label>From <input id="time-low" class="range-input" type="number" min="0" step="0.01" inputmode="decimal"><span>ms</span></label><span aria-hidden="true">–</span><label>To <input id="time-high" class="range-input" type="number" min="0" step="0.01" inputmode="decimal"><span>ms</span></label></div></div></div>
    <p class="muted">Invocation bars and sampled work are shown beneath the selected time window. Drag across the lanes to draw a new range, or drag the capture-start strip to move it.</p>
    <div id="timeline-scroll" class="timeline-scroll"><div id="timeline-labels-scroll" class="timeline-labels-scroll"><svg id="timeline-labels" aria-hidden="true"></svg></div><div id="timeline-chart-scroll" class="timeline-chart-scroll"><svg id="timeline" role="img" aria-label="Capture timeline with one lane per thread"></svg></div></div>
  </section>
  <section class="controls" aria-label="Profile controls">
    <fieldset><legend><span>Invocation latency</span><span id="percentile-label" class="range-editor percentile-editor"><span aria-hidden="true">p</span><input id="pct-low" class="range-input percentile-input" type="number" min="0" max="100" step="1" aria-label="Lower latency percentile"><span aria-hidden="true">: p</span><input id="pct-high" class="range-input percentile-input" type="number" min="0" max="100" step="1" aria-label="Upper latency percentile"></span><label class="histogram-bucket-control">Bucket <select id="histogram-bucket-size" class="histogram-bucket-select" aria-label="Histogram bucket size"><option value="auto">Auto</option><option value="250000">0.25 ms</option><option value="500000">0.50 ms</option><option value="1000000">1.00 ms</option><option value="2000000">2.00 ms</option><option value="5000000">5.00 ms</option></select></label></legend><svg id="histogram" viewBox="0 0 720 126" preserveAspectRatio="none" role="img" aria-label="Invocation latency histogram"></svg></fieldset>
    <fieldset><legend>Metric</legend><select id="metric"><option value="wall">Wall time</option><option value="cpu">CPU time</option><option value="off_cpu">Off-CPU time</option></select><p class="control-help">Wall latency selects invocations; metric controls flame widths.</p></fieldset>
  </section>
  <section class="summary" aria-live="polite"><div><p>Selected invocations</p><strong id="selected-count"></strong><span id="latency-range"></span></div><div><p>Selected samples</p><strong id="sample-count"></strong><span id="sample-period"></span></div><div><p>Sampled CPU / off-CPU</p><strong id="cpu-time"></strong><span id="offcpu-time"></span></div></section>
  <section class="flame-section"><div class="section-heading"><div><p class="label">Selected execution paths</p><h2>Flame graph</h2></div><div class="flame-actions"><label class="search"><span class="sr-only">Search frames</span><input id="frame-search" type="search" placeholder="Search frames"></label></div></div><nav id="flame-zoom-path" class="flame-zoom-path" aria-label="Flame graph zoom path"></nav><p class="muted">Starts at the named function and shows sampled descendants for the selected invocations and metric; callers above the population selector are intentionally omitted. Unsampled or inlined code cannot appear. Hover for details; click to zoom.</p><svg id="flame" role="img" aria-label="Interactive flame graph"></svg><div id="empty" hidden>No samples match this query.</div><div id="flame-tooltip" class="tooltip" role="status" aria-live="polite" hidden></div></section>
</main>
<script id="slice-profile" type="application/json">{}</script>
<script id="slice-initial-query" type="application/json">{}</script>
<script>{}</script>
</body></html>"#,
        CSS, profile_json, initial_json, JAVASCRIPT
    ))
}

fn script_safe_json(value: &impl Serialize) -> Result<String, RenderError> {
    Ok(serde_json::to_string(value)?
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e"))
}

const CSS: &str = r##"
:root { color-scheme: light; font-family: "Space Grotesk", "Arial Black", ui-sans-serif, system-ui, sans-serif; background:#f4f0e8; color:#171717; --ink:#171717; --paper:#fffaf0; --pink:#ff4f9a; --yellow:#ffd447; --cyan:#5ce1e6; --green:#b8f35b; --purple:#a98cff; }
* { box-sizing:border-box; } body { margin:0; background-color:#f4f0e8; background-image:radial-gradient(#171717 1px,transparent 1px); background-size:18px 18px; } main { max-width:1440px; margin:auto; padding:28px clamp(16px,4vw,52px) 72px; }
h1,h2,p { margin:0; } h1 { font-size:clamp(2.5rem,6vw,5.2rem); line-height:.88; letter-spacing:-.08em; text-transform:uppercase; } h2 { font-size:1.25rem; letter-spacing:-.04em; text-transform:uppercase; } .label { color:var(--ink); font-size:.7rem; font-weight:950; letter-spacing:.13em; } .muted { color:#454545; font-size:.83rem; }
.topbar { display:flex; align-items:end; justify-content:space-between; gap:18px; border-bottom:4px solid var(--ink); padding-bottom:19px; }
.viewer-error { margin:14px 0; padding:10px 12px; border:3px solid var(--ink); background:var(--pink); color:var(--ink); box-shadow:5px 5px 0 var(--ink); font-size:.82rem; font-weight:800; }
.population { display:flex; justify-content:space-between; gap:18px; padding:29px 0 22px; } .population h2 { display:inline-block; margin-top:8px; padding:6px 9px; border:3px solid var(--ink); background:var(--cyan); box-shadow:4px 4px 0 var(--ink); font-size:1.35rem; } .population .muted { margin-top:14px; } .capture-meta { display:flex; flex-direction:column; gap:5px; color:#454545; font-size:.75rem; font-weight:700; text-align:right; font-variant-numeric:tabular-nums; }
.timeline-section,.flame-section { border:3px solid var(--ink); border-radius:0; background:var(--paper); box-shadow:8px 8px 0 var(--ink); padding:18px; } .timeline-section { margin-bottom:18px; } .section-heading { display:flex; justify-content:space-between; align-items:center; gap:14px; margin-bottom:8px; } .timeline-actions { display:flex; align-items:center; justify-content:flex-end; gap:12px; min-width:0; } .value { color:var(--ink); font-variant-numeric:tabular-nums; } .timeline-scroll { height:280px; max-width:100%; display:grid; grid-template-columns:170px minmax(0,1fr); overflow:hidden; margin-top:14px; border:3px solid var(--ink); background:var(--paper); box-shadow:5px 5px 0 var(--ink); overscroll-behavior:contain; } .timeline-labels-scroll,.timeline-chart-scroll { min-width:0; min-height:0; overflow-y:auto; overflow-x:hidden; scrollbar-width:none; } .timeline-labels-scroll::-webkit-scrollbar { width:0; height:0; } .timeline-chart-scroll { overflow:auto; } #timeline-labels,#timeline { display:block; background:var(--paper); } #timeline-labels { width:170px; } #timeline { min-height:120px; touch-action:none; } .timeline-bg { fill:var(--paper); } .timeline-lane { fill:#ece7dc; } .timeline-selection { fill:var(--yellow); opacity:.84; stroke:var(--ink); stroke-width:4; vector-effect:non-scaling-stroke; pointer-events:none; } .timeline-invocation { fill:var(--purple); opacity:.88; cursor:pointer; } .timeline-invocation.selected { fill:var(--pink); opacity:1; } .timeline-invocation:hover { stroke:var(--ink); stroke-width:2; } .timeline-sample { fill:#31bfc5; opacity:.7; pointer-events:none; } .timeline-text,.timeline-axis { fill:var(--ink); font-size:10px; dominant-baseline:middle; } .timeline-axis { font-size:9px; font-weight:800; } .thread-label { cursor:pointer; } .thread-label.inactive { fill:#999; text-decoration:line-through; } .time-handle { stroke:var(--pink); stroke-width:4; pointer-events:none; vector-effect:non-scaling-stroke; } .time-handle-hit { fill:transparent; cursor:ew-resize; }
.controls { display:grid; grid-template-columns:minmax(0,2.8fr) minmax(180px,1fr); gap:18px; margin:18px 0; } fieldset { min-width:0; margin:0; border:3px solid var(--ink); border-radius:0; background:var(--paper); box-shadow:6px 6px 0 var(--ink); padding:13px; } legend { display:flex; align-items:center; gap:10px; color:var(--ink); font-size:.8rem; font-weight:900; padding:0 5px; white-space:nowrap; } input,select,button { accent-color:var(--pink); } .range-editor { display:flex; align-items:center; gap:5px; color:var(--ink); font-variant-numeric:tabular-nums; white-space:nowrap; } #time-label { padding:6px 8px; border:3px solid var(--ink); background:var(--yellow); box-shadow:4px 4px 0 var(--ink); } .range-editor label { display:flex; align-items:center; gap:4px; color:var(--ink); font-size:.76rem; font-weight:800; } .range-input { width:108px; min-width:108px; background:#fff; color:var(--ink); border:2px solid var(--ink); border-radius:0; padding:5px 6px; font:inherit; font-weight:700; font-variant-numeric:tabular-nums; } .percentile-editor { display:inline-flex; gap:7px; margin-left:4px; font-size:.8rem; } .percentile-input { width:64px; min-width:64px; text-align:center; padding:3px 2px; } select { width:100%; background:#fff; border:2px solid var(--ink); border-radius:0; color:var(--ink); padding:6px; font-weight:700; } .histogram-bucket-control { display:flex; align-items:center; gap:5px; color:var(--ink); font-size:.76rem; font-weight:800; } .histogram-bucket-select { width:auto; min-width:84px; padding:3px 5px; font-size:.76rem; } .thread-picker { position:relative; flex:0 1 auto; min-width:180px; } .thread-picker summary { display:flex; align-items:center; justify-content:space-between; gap:12px; cursor:pointer; list-style:none; border:3px solid var(--ink); background:var(--cyan); box-shadow:4px 4px 0 var(--ink); color:var(--ink); padding:6px 8px; font-size:.78rem; font-weight:800; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; } .thread-picker summary::after { content:"▾"; flex:0 0 auto; font-size:1rem; line-height:1; } .thread-picker summary::-webkit-details-marker { display:none; } .thread-picker[open] summary { box-shadow:none; } .thread-list { position:absolute; z-index:3; left:0; right:0; display:grid; gap:4px; max-height:180px; overflow:auto; margin-top:3px; padding:7px; border:3px solid var(--ink); background:var(--paper); box-shadow:4px 4px 0 var(--ink); } .thread-list label { display:flex; gap:6px; align-items:center; padding:4px 5px; font-size:.74rem; font-weight:700; white-space:nowrap; cursor:pointer; } .control-help { color:#454545; font-size:.74rem; margin-top:10px; line-height:1.35; }
#histogram { width:100%; height:86px; display:block; margin-bottom:4px; touch-action:none; } .histogram-rail { fill:#e2dccf; } .histogram-rail-selection { fill:#c9bfad; opacity:.95; stroke:var(--ink); stroke-width:3; } .hist-bar { fill:var(--cyan); } .hist-bar.hist-empty { fill:#d9d1c3; stroke:#b8aa96; stroke-width:1; } .hist-selected { fill:var(--pink); } .hist-label { fill:var(--ink); font-size:10px; font-weight:800; } .hist-axis-tick { stroke:#8c8170; stroke-width:1; } .hist-handle { stroke:var(--ink); stroke-width:3; stroke-dasharray:4 2; pointer-events:none; } .hist-handle-hit { fill:transparent; cursor:ew-resize; } .hist-window { fill:var(--yellow); opacity:.72; stroke:var(--ink); stroke-width:3; cursor:grab; }
.timeline-chart-scroll { overflow-x:scroll; overflow-y:auto; scrollbar-width:auto; }
.flame-zoom-path { display:flex; align-items:center; flex-wrap:wrap; gap:5px; min-height:28px; margin:4px 0 8px; color:var(--ink); font-size:.76rem; } .flame-zoom-path button { padding:3px 7px; background:var(--cyan); border:2px solid var(--ink); font-size:.74rem; } .flame-zoom-path button[aria-current="page"] { color:var(--ink); background:var(--yellow); } .flame-zoom-separator { color:var(--ink); font-weight:900; }
.summary { display:grid; grid-template-columns:repeat(3,1fr); gap:18px; margin:18px 0; } .summary>div { background:var(--pink); border:3px solid var(--ink); border-radius:0; box-shadow:6px 6px 0 var(--ink); padding:12px 15px; } .summary>div:nth-child(2) { background:var(--green); } .summary>div:nth-child(3) { background:var(--cyan); } .summary p { color:var(--ink); font-size:.76rem; font-weight:800; } .summary strong { display:block; margin-top:4px; font-size:1.2rem; font-variant-numeric:tabular-nums; } .summary span { display:block; margin-top:3px; color:var(--ink); font-size:.74rem; }
.flame-actions { display:flex; align-items:center; gap:8px; } .search input { width:190px; background:#fff; color:var(--ink); border:2px solid var(--ink); border-radius:0; padding:6px 8px; } button { color:var(--ink); background:var(--cyan); border:2px solid var(--ink); padding:6px 9px; border-radius:0; box-shadow:3px 3px 0 var(--ink); cursor:pointer; font:inherit; font-weight:800; } button:hover { background:var(--yellow); transform:translate(-1px,-1px); box-shadow:4px 4px 0 var(--ink); } #flame { width:100%; min-height:160px; display:block; margin-top:12px; border:3px solid var(--ink); background:#fff; } .frame { stroke:var(--ink); stroke-width:2; cursor:pointer; } .frame:hover,.frame.match { stroke:#fff; stroke-width:3; } .frame-label { fill:var(--ink); pointer-events:none; font-size:11px; font-weight:800; dominant-baseline:middle; } #empty { color:var(--ink); padding:38px; text-align:center; font-weight:800; } .tooltip { position:fixed; z-index:10; max-width:330px; pointer-events:none; background:var(--yellow); border:3px solid var(--ink); box-shadow:5px 5px 0 var(--ink); padding:10px 12px; font-size:.77rem; line-height:1.5; } .tooltip strong { color:var(--ink); display:block; margin-bottom:3px; } .tooltip span { color:var(--ink); display:block; } .sr-only { position:absolute; width:1px; height:1px; padding:0; margin:-1px; overflow:hidden; clip:rect(0,0,0,0); white-space:nowrap; border:0; }
@media (max-width:900px) { .controls { grid-template-columns:1fr; } } @media (max-width:620px) { .topbar,.population { align-items:start; flex-direction:column; } .capture-meta { text-align:left; } .controls,.summary { grid-template-columns:1fr; } .timeline-actions { align-items:stretch; flex-direction:column; } .thread-picker { width:100%; } .flame-actions { align-items:stretch; flex-direction:column; } .search input { width:100%; } }
"##;

const JAVASCRIPT: &str = r##"
(() => {
  'use strict';
  try {
  const profile = JSON.parse(document.getElementById('slice-profile').textContent);
  const initial = JSON.parse(document.getElementById('slice-initial-query').textContent);
  const byStack = new Map(profile.stacks.map(stack => [stack.id, stack]));
  const byFunction = new Map(profile.functions.map(fn => [fn.id, fn]));
  const times = profile.invocations.flatMap(invocation => [invocation.start_ns, invocation.end_ns]).concat(profile.samples.map(sample => sample.timestamp_ns));
  const captureFrom = times.length ? Math.min(...times) : 0;
  const captureTo = times.length ? Math.max(...times) : 1;
  const bounds = {from:captureFrom, to:Math.max(captureFrom + 1, captureTo)};
  const threadRows = profile.threads.length ? profile.threads : [...new Set(profile.invocations.map(invocation => invocation.tid))].sort().map(tid => ({tid,name:null}));
  const allThreadIds = threadRows.map(thread => thread.tid);
  const initialTime = initial.time || {from_ns:bounds.from, to_ns:bounds.to};
  const clampTime = value => Math.max(bounds.from, Math.min(bounds.to, value));
  const state = { functionId:initial.function_id, threads:new Set(initial.threads || allThreadIds), time:{from_ns:clampTime(initialTime.from_ns),to_ns:clampTime(initialTime.to_ns)}, percentile:initial.percentile || {low:0,high:100}, latency:null, histogramBucketSizeNs:null, metric:initial.metric || 'wall', search:'' };
  if (state.time.from_ns >= state.time.to_ns) { state.time.from_ns=bounds.from; state.time.to_ns=bounds.to; }
  let zoomPath = [];
  let drag = null;
  let timelineScale = 1;
  let histogramView = null;
  const id = name => document.getElementById(name);
  const ns = value => `${(value / 1e6).toFixed(value >= 1e9 ? 0 : 2)} ms`;
  const relativeMs = value => ((value-bounds.from)/1e6).toFixed(2);
  const count = value => value.toLocaleString();
  const escapeHtml = value => String(value).replace(/[&<>"']/g, character => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[character]));
  const svgEl = (name, attributes = {}) => { const element=document.createElementNS('http://www.w3.org/2000/svg',name); Object.entries(attributes).forEach(([key,value])=>element.setAttribute(key,String(value))); return element; };
  const rankBounds = (size, range) => [Math.min(size,Math.ceil(size*range.low/100)), Math.min(size,Math.ceil(size*range.high/100))];
  const metricIncludes = stateName => state.metric==='wall' || (state.metric==='cpu' && stateName==='on_cpu') || (state.metric==='off_cpu' && stateName==='off_cpu');
  const position = value => (value-bounds.from)/Math.max(1,bounds.to-bounds.from);
  const invocationsByThread = new Map(allThreadIds.map(tid => [tid, []]));
  profile.invocations.forEach(invocation => { if(invocation.complete && invocation.valid && invocationsByThread.has(invocation.tid)) invocationsByThread.get(invocation.tid).push(invocation); });
  const samplesByThread = new Map();
  profile.samples.forEach(sample => { const bucket=Math.max(0,Math.min(79,Math.floor(position(sample.timestamp_ns)*80))); if(!samplesByThread.has(sample.tid)) samplesByThread.set(sample.tid,Array(80).fill(0)); samplesByThread.get(sample.tid)[bucket]+=sample.weight_ns; });
  const timelineValue = (event,svg,width,labelWidth,chartWidth) => { const rect=svg.getBoundingClientRect(), local=(event.clientX-rect.left)/rect.width*width, ratio=Math.max(0,Math.min(1,(local-labelWidth)/chartWidth)); return bounds.from+ratio*(bounds.to-bounds.from); };
  const percentileValue = (event,svg,width,min,max) => { const rect=svg.getBoundingClientRect(), x=Math.max(0,Math.min(width,(event.clientX-rect.left)/rect.width*width)); return min+(x/width)*Math.max(1,max-min); };
  const percentileAtDuration = (durations,value,side='low') => { if(durations.length<2) return 0; if(value<=durations[0]) return 0; if(value>=durations[durations.length-1]) return 100; if(side==='low') { let index=1; while(index<durations.length&&durations[index]<value) index++; if(durations[index]===value) return index/(durations.length-1)*100; const low=durations[index-1], high=durations[index]; return (index-1+(value-low)/Math.max(1,high-low))/(durations.length-1)*100; } let index=1; while(index<durations.length&&durations[index]<=value) index++; if(durations[index-1]===value) return index/(durations.length-1)*100; const low=durations[index-1], high=durations[index]; return (index-1+(value-low)/Math.max(1,high-low))/(durations.length-1)*100; };
  const durationAtPercentile = (durations,percentile) => { if(!durations.length) return 0; if(durations.length===1) return durations[0]; const rank=percentile/100*(durations.length-1), low=Math.floor(rank), high=Math.ceil(rank); return durations[low]+(durations[high]-durations[low])*(rank-low); };
  const clampPercentileWindow = (low,high) => { low=Math.max(0,Math.min(99,Math.round(low))); high=Math.max(1,Math.min(100,Math.round(high))); if(low>=high) { if(drag && drag.edge==='low') low=high-1; else high=low+1; } return {low,high}; };
  function eligible() { return profile.invocations.filter(invocation => invocation.function_id===state.functionId && invocation.complete && invocation.valid && state.threads.has(invocation.tid) && invocation.start_ns>=state.time.from_ns && invocation.start_ns<state.time.to_ns).sort((a,b)=>(a.end_ns-a.start_ns)-(b.end_ns-b.start_ns) || a.id-b.id); }
  function syncLatencyWindow(durations) {
    if(!durations.length) { state.latency={low_ns:0,high_ns:0}; return {low:0,high:0}; }
    if(!state.latency) state.latency={low_ns:durationAtPercentile(durations,state.percentile.low),high_ns:durationAtPercentile(durations,state.percentile.high)};
    const low=Math.max(durations[0],Math.min(durations[durations.length-1],state.latency.low_ns)), high=Math.max(low,Math.min(durations[durations.length-1],state.latency.high_ns));
    state.latency={low_ns:low,high_ns:high};
    if(low<high) state.percentile={low:percentileAtDuration(durations,low,'low'),high:percentileAtDuration(durations,high,'high')};
    return {low,high};
  }
  function node(name) { return {name,value:0,selfValue:0,sampleCount:0,invocations:new Set(),children:new Map(),cpu:0,off:0}; }
  function calculate() {
    const all=eligible(), durations=all.map(invocation=>invocation.end_ns-invocation.start_ns), selectedFunction=byFunction.get(state.functionId), selectedName=selectedFunction && selectedFunction.demangled_name;
    const {low:lowValue,high:highValue}=syncLatencyWindow(durations);
    const [start,end]=rankBounds(all.length,state.percentile), chosen=all.slice(start,end), selectedIds=new Set(chosen.map(invocation=>invocation.id)), root=node('root');
    let selectedSampleCount=0, cpu=0, off=0;
    for (const sample of profile.samples) {
      if (!selectedIds.has(sample.invocation_id)) continue;
      const stack=byStack.get(sample.stack_id); if (!stack) continue;
      const startFrame=stack.frames.findIndex(frame=>frame.function_id===state.functionId || frame.label===selectedName); if(startFrame<0) continue;
      const included=metricIncludes(sample.state); if (included) selectedSampleCount++;
      if (sample.state==='on_cpu') cpu+=sample.weight_ns; else off+=sample.weight_ns;
      let current=root; current.sampleCount++; current.invocations.add(sample.invocation_id); sample.state==='on_cpu' ? current.cpu+=sample.weight_ns : current.off+=sample.weight_ns; if (included) current.value+=sample.weight_ns;
      stack.frames.slice(startFrame).forEach((frame,index) => { if(!current.children.has(frame.label)) current.children.set(frame.label,node(frame.label)); current=current.children.get(frame.label); current.sampleCount++; current.invocations.add(sample.invocation_id); sample.state==='on_cpu' ? current.cpu+=sample.weight_ns : current.off+=sample.weight_ns; if (included) { current.value+=sample.weight_ns; if(index===stack.frames.length-startFrame-1) current.selfValue+=sample.weight_ns; } });
    }
    return {all,chosen,selectedIds,root,selectedSampleCount,cpu,off,low:lowValue,high:highValue,rankStart:start,rankEnd:end,durations};
  }
  /* Superseded interaction implementation retained only in source history.
  function paintTimeline(result) {
    const svg=id('timeline'), labels=id('timeline-labels'), labelScroll=id('timeline-labels-scroll'), chartScroll=id('timeline-chart-scroll'), viewportWidth=chartScroll.clientWidth||800, width=Math.max(240,viewportWidth*timelineScale), chartWidth=width, row=27, axis=24, height=Math.max(96,axis+threadRows.length*row+10); svg.innerHTML=''; labels.innerHTML=''; svg.style.width=`${width}px`; svg.style.height=`${height}px`; labels.style.height=`${height}px`; svg.setAttribute('viewBox',`0 0 ${width} ${height}`); labels.setAttribute('viewBox',`0 0 170 ${height}`); labels.appendChild(svgEl('rect',{class:'timeline-bg',x:0,y:0,width:170,height,rx:6})); svg.appendChild(svgEl('rect',{class:'timeline-bg',x:0,y:0,width,height,rx:6}));
    const selectionX=position(state.time.from_ns)*chartWidth, selectionWidth=Math.max(1,(position(state.time.to_ns)-position(state.time.from_ns))*chartWidth); svg.appendChild(svgEl('rect',{class:'timeline-selection',x:selectionX,y:0,width:selectionWidth,height}));
    const appendText=(target,text,x,y,className='timeline-text')=>{const element=svgEl('text',{class:className,x,y}); element.textContent=text; target.appendChild(element); return element;};
    appendText(labels,'Threads',6,11,'timeline-axis'); appendText(svg,'capture start',0,11,'timeline-axis'); appendText(svg,ns(bounds.to-bounds.from),width-80,11,'timeline-axis');
    const samplesByThread=new Map(); for(const sample of profile.samples) { const bucket=Math.max(0,Math.min(79,Math.floor(position(sample.timestamp_ns)*80))); if(!samplesByThread.has(sample.tid)) samplesByThread.set(sample.tid,Array(80).fill(0)); samplesByThread.get(sample.tid)[bucket]+=sample.weight_ns; }
    const maxDensity=Math.max(1,...[...samplesByThread.values()].flat());
    const toggleThread=tid=>{ state.threads.has(tid)?state.threads.delete(tid):state.threads.add(tid); render(); };
    threadRows.forEach((thread,index)=>{ const y=axis+index*row; svg.appendChild(svgEl('rect',{class:'timeline-lane',x:0,y,width:chartWidth,height:row-2,rx:2})); labels.appendChild(svgEl('rect',{class:'timeline-lane',x:0,y,width:170,height:row-2,rx:2})); const label=appendText(labels,`${thread.name || 'TID'} ${thread.tid}`,6,y+12,`timeline-text thread-label${state.threads.has(thread.tid)?'':' inactive'}`); label.setAttribute('tabindex','0'); label.setAttribute('role','button'); label.setAttribute('aria-pressed',String(state.threads.has(thread.tid))); label.addEventListener('click',()=>toggleThread(thread.tid)); label.addEventListener('keydown',event=>{if(event.key==='Enter'||event.key===' '){event.preventDefault(); toggleThread(thread.tid);}}); const density=samplesByThread.get(thread.tid)||[]; density.forEach((value,bucket)=>{ if(!value) return; svg.appendChild(svgEl('rect',{class:'timeline-sample',x:bucket/80*chartWidth,y:y+row-7,width:Math.max(1,chartWidth/80),height:4+value/maxDensity*8})); }); profile.invocations.filter(invocation=>invocation.tid===thread.tid && invocation.complete && invocation.valid).forEach(invocation=>{ const x=position(invocation.start_ns)*chartWidth; const w=Math.max(2,(invocation.end_ns-invocation.start_ns)/Math.max(1,bounds.to-bounds.from)*chartWidth); const bar=svgEl('rect',{class:`timeline-invocation${result.selectedIds.has(invocation.id)?' selected':''}`,x,y:y+4,width:Math.min(w,chartWidth),height:row-12,rx:2}); bar.addEventListener('mouseenter',event=>showTimelineTooltip(event,invocation)); bar.addEventListener('mouseleave',hideTooltip); svg.appendChild(bar); }); });
    [0,.25,.5,.75,1].forEach(ratio=>appendText(svg,ns((bounds.to-bounds.from)*ratio),ratio*chartWidth,axis+threadRows.length*row+1,'timeline-axis'));
    const begin=(kind,event)=>{ drag={kind,pointerId:event.pointerId,startX:event.clientX,from:state.time.from_ns,to:state.time.to_ns}; svg.setPointerCapture(event.pointerId); event.preventDefault(); };
    const handle=(kind,value)=>{ const x=position(value)*chartWidth, line=svgEl('line',{class:'time-handle',x1:x,x2:x,y1:0,y2:height,'data-handle':kind}), hit=svgEl('rect',{class:'time-handle-hit',x:x-8,y:0,width:16,height, 'data-handle':kind}); line.style.stroke='#d7efff'; line.style.strokeWidth='2'; hit.addEventListener('pointerdown',event=>{event.stopPropagation(); begin(kind,event);}); svg.appendChild(line); svg.appendChild(hit); }; handle('time-from',state.time.from_ns); handle('time-to',state.time.to_ns);
    svg.onpointerdown=event=>{ const classes=event.target.classList, value=timelineValue(event,svg,width,0,chartWidth); if(!classes.contains('timeline-axis')&&!classes.contains('time-handle-hit')&&value>=state.time.from_ns&&value<=state.time.to_ns) begin('time-move',event); };
    svg.onpointermove=event=>{ if(!drag || drag.pointerId!==event.pointerId) return; const value=timelineValue(event,svg,width,0,chartWidth), span=bounds.to-bounds.from, delta=(event.clientX-drag.startX)/svg.getBoundingClientRect().width*width/chartWidth*span; if(drag.kind==='time-from') state.time.from_ns=Math.min(value,state.time.to_ns-1); else if(drag.kind==='time-to') state.time.to_ns=Math.max(value,state.time.from_ns+1); else { const shift=Math.max(bounds.from-drag.from,Math.min(bounds.to-drag.to,delta)); state.time.from_ns=drag.from+shift; state.time.to_ns=drag.to+shift; } render(); };
    svg.onpointerup=event=>{if(drag && drag.pointerId===event.pointerId){drag=null; svg.releasePointerCapture(event.pointerId);}}; svg.onpointercancel=svg.onpointerup;
    svg.onwheel=event=>{ const value=timelineValue(event,svg,width,0,chartWidth), chartRect=chartScroll.getBoundingClientRect(), pointerX=event.clientX-chartRect.left, nextScale=Math.max(1,Math.min(8,timelineScale*Math.exp(-event.deltaY*0.002))); if(value<state.time.from_ns||value>state.time.to_ns||nextScale===timelineScale) return; event.preventDefault(); timelineScale=nextScale; render(); const nextWidth=Math.max(240,chartScroll.clientWidth*timelineScale); chartScroll.scrollLeft=Math.max(0,position(value)*nextWidth-pointerX); };
    chartScroll.onscroll=()=>{if(labelScroll.scrollTop!==chartScroll.scrollTop) labelScroll.scrollTop=chartScroll.scrollTop;}; labelScroll.onscroll=()=>{if(chartScroll.scrollTop!==labelScroll.scrollTop) chartScroll.scrollTop=labelScroll.scrollTop;}; labelScroll.scrollTop=chartScroll.scrollTop;
  }
  function paintHistogramSuperseded(result) {
    const all=result.all, svg=id('histogram'), width=720, height=126, bins=30; svg.innerHTML=''; if(!all.length) return; const durations=all.map(invocation=>invocation.end_ns-invocation.start_ns), min=Math.min(...durations), max=Math.max(...durations), span=Math.max(1,max-min), counts=Array(bins).fill(0), selected=Array(bins).fill(false), [start,end]=rankBounds(all.length,state.percentile); all.forEach((invocation,index)=>{const bucket=Math.min(bins-1,Math.floor((invocation.end_ns-invocation.start_ns-min)/span*bins)); counts[bucket]++; if(index>=start&&index<end) selected[bucket]=true;}); const peak=Math.max(1,...counts); counts.forEach((value,index)=>{const x=index*width/bins, h=value/peak*92, rect=svgEl('rect',{class:selected[index]?'hist-selected':'hist-bar',x:x+1,y:98-h,width:width/bins-2,height:h,rx:2}); svg.appendChild(rect);}); const xFor=value=>Math.max(0,Math.min(width,(value-min)/span*width)), lowValue=result.low ?? min, highValue=result.high ?? max, lowX=xFor(lowValue), highX=xFor(highValue); svg.appendChild(svgEl('rect',{class:'hist-window',x:lowX,y:0,width:Math.max(2,highX-lowX),height:103})); const label=(text,x)=>{const node=svgEl('text',{class:'hist-label',x,y:119}); node.textContent=text; svg.appendChild(node);}; label(ns(min),0); label(ns(max),width-66); const line=(x,text,edge)=>{const node=svgEl('line',{class:'hist-handle',x1:x,x2:x,y1:0,y2:102,'data-edge':edge}), hit=svgEl('rect',{class:'hist-handle-hit',x:x-8,y:0,width:16,height:103}); node.style.pointerEvents='none'; hit.addEventListener('pointerdown',event=>{event.stopPropagation(); drag={kind:`latency-${edge}`,edge,pointerId:event.pointerId,startX:event.clientX,low:state.latency.low_ns,high:state.latency.high_ns,startDuration:percentileValue(event,svg,width,min,max)}; svg.setPointerCapture(event.pointerId); event.preventDefault();}); svg.appendChild(node); svg.appendChild(hit);}; line(lowX,`p${state.percentile.low}`,'low'); line(highX,`p${state.percentile.high}`,'high'); svg.onpointerdown=event=>{if(event.target.tagName!=='text') {drag={kind:'latency-move',pointerId:event.pointerId,startX:event.clientX,low:state.latency.low_ns,high:state.latency.high_ns,startDuration:percentileValue(event,svg,width,min,max)}; svg.setPointerCapture(event.pointerId); event.preventDefault();}}; svg.onpointermove=event=>{if(!drag||drag.pointerId!==event.pointerId||!drag.kind.startsWith('latency-')) return; const next=percentileValue(event,svg,width,min,max); if(drag.kind==='latency-low') state.latency.low_ns=Math.min(next,state.latency.high_ns); else if(drag.kind==='latency-high') state.latency.high_ns=Math.max(next,state.latency.low_ns); else {const shift=Math.max(min-drag.low,Math.min(max-drag.high,next-drag.startDuration)); state.latency.low_ns=drag.low+shift; state.latency.high_ns=drag.high+shift;} render();}; svg.onpointerup=event=>{if(drag&&drag.pointerId===event.pointerId){drag=null;svg.releasePointerCapture(event.pointerId);}}; svg.onpointercancel=svg.onpointerup;
  }
  */
  // Fast interaction paths. Dragging updates only the active SVG geometry;
  // the expensive population/flame aggregation is committed on pointerup.
  function paintTimeline(result) {
    const svg=id('timeline'), labels=id('timeline-labels'), labelScroll=id('timeline-labels-scroll'), chartScroll=id('timeline-chart-scroll'), viewportWidth=chartScroll.clientWidth||800, width=Math.max(240,viewportWidth*timelineScale), chartWidth=width, row=27, axis=24, height=Math.max(96,axis+threadRows.length*row+10); svg.innerHTML=''; labels.innerHTML=''; svg.style.width=`${width}px`; svg.style.height=`${height}px`; labels.style.height=`${height}px`; svg.setAttribute('viewBox',`0 0 ${width} ${height}`); labels.setAttribute('viewBox',`0 0 170 ${height}`); labels.appendChild(svgEl('rect',{class:'timeline-bg',x:0,y:0,width:170,height,rx:6})); svg.appendChild(svgEl('rect',{class:'timeline-bg',x:0,y:0,width,height,rx:6}));
    const appendText=(target,text,x,y,className='timeline-text')=>{const element=svgEl('text',{class:className,x,y}); element.textContent=text; target.appendChild(element); return element;};
    const updateWindow=()=>{const fromX=position(state.time.from_ns)*chartWidth, toX=position(state.time.to_ns)*chartWidth, selection=svg.querySelector('.timeline-selection'); if(selection){selection.setAttribute('x',fromX);selection.setAttribute('width',Math.max(1,toX-fromX));} [['time-from',fromX],['time-to',toX]].forEach(([kind,x])=>svg.querySelectorAll(`[data-handle="${kind}"]`).forEach(element=>{element.setAttribute('x1',x);element.setAttribute('x2',x);element.setAttribute('x',x-8);})); id('time-low').value=relativeMs(state.time.from_ns); id('time-high').value=relativeMs(state.time.to_ns); };
    const selectionX=position(state.time.from_ns)*chartWidth, selectionWidth=Math.max(1,(position(state.time.to_ns)-position(state.time.from_ns))*chartWidth); svg.appendChild(svgEl('rect',{class:'timeline-selection',x:selectionX,y:0,width:selectionWidth,height})); appendText(labels,'Threads',6,11,'timeline-axis'); appendText(svg,'capture start',0,11,'timeline-axis'); appendText(svg,ns(bounds.to-bounds.from),width-80,11,'timeline-axis');
    const maxDensity=Math.max(1,...[...samplesByThread.values()].flat()); const toggleThread=tid=>{state.threads.has(tid)?state.threads.delete(tid):state.threads.add(tid);render();};
    threadRows.forEach((thread,index)=>{const y=axis+index*row; svg.appendChild(svgEl('rect',{class:'timeline-lane',x:0,y,width:chartWidth,height:row-2,rx:2})); labels.appendChild(svgEl('rect',{class:'timeline-lane',x:0,y,width:170,height:row-2,rx:2})); const label=appendText(labels,`${thread.name || 'TID'} ${thread.tid}`,6,y+12,`timeline-text thread-label${state.threads.has(thread.tid)?'':' inactive'}`); label.setAttribute('tabindex','0'); label.setAttribute('role','button'); label.setAttribute('aria-pressed',String(state.threads.has(thread.tid))); label.addEventListener('click',()=>toggleThread(thread.tid)); label.addEventListener('keydown',event=>{if(event.key==='Enter'||event.key===' '){event.preventDefault();toggleThread(thread.tid);}}); const density=samplesByThread.get(thread.tid)||[]; density.forEach((value,bucket)=>{if(value) svg.appendChild(svgEl('rect',{class:'timeline-sample',x:bucket/80*chartWidth,y:y+row-7,width:Math.max(1,chartWidth/80),height:4+value/maxDensity*8}));}); (invocationsByThread.get(thread.tid)||[]).forEach(invocation=>{const x=position(invocation.start_ns)*chartWidth, w=Math.max(2,(invocation.end_ns-invocation.start_ns)/Math.max(1,bounds.to-bounds.from)*chartWidth), bar=svgEl('rect',{class:`timeline-invocation${result.selectedIds.has(invocation.id)?' selected':''}`,x,y:y+4,width:Math.min(w,chartWidth),height:row-12,rx:2}); bar.addEventListener('mouseenter',event=>showTimelineTooltip(event,invocation)); bar.addEventListener('mouseleave',hideTooltip); svg.appendChild(bar);});});
    [0,.25,.5,.75,1].forEach(ratio=>appendText(svg,ns((bounds.to-bounds.from)*ratio),ratio*chartWidth,axis+threadRows.length*row+1,'timeline-axis')); const begin=(kind,event)=>{const rect=svg.getBoundingClientRect();drag={kind,pointerId:event.pointerId,startX:event.clientX,startValue:timelineValue(event,svg,width,0,chartWidth),from:state.time.from_ns,to:state.time.to_ns,rect};svg.setPointerCapture(event.pointerId);event.preventDefault();}; const handle=(kind,value)=>{const x=position(value)*chartWidth,line=svgEl('line',{class:'time-handle',x1:x,x2:x,y1:0,y2:height,'data-handle':kind}),hit=svgEl('rect',{class:'time-handle-hit',x:x-8,y:0,width:16,height,'data-handle':kind});hit.addEventListener('pointerdown',event=>{event.stopPropagation();begin(kind,event);});svg.appendChild(line);svg.appendChild(hit);}; handle('time-from',state.time.from_ns);handle('time-to',state.time.to_ns);
    svg.onpointerdown=event=>{const classes=event.target.classList;if(classes.contains('time-handle-hit'))return;const rect=svg.getBoundingClientRect(),inAxis=event.clientY-rect.top<axis;begin(inAxis?'time-move':'time-select',event);}; svg.onpointermove=event=>{if(!drag||drag.pointerId!==event.pointerId)return;const value=timelineValue(event,svg,width,0,chartWidth);if(drag.kind==='time-from')state.time.from_ns=Math.min(value,state.time.to_ns-1);else if(drag.kind==='time-to')state.time.to_ns=Math.max(value,state.time.from_ns+1);else if(drag.kind==='time-move'){const delta=value-drag.startValue,shift=Math.max(bounds.from-drag.from,Math.min(bounds.to-drag.to,delta));state.time.from_ns=drag.from+shift;state.time.to_ns=drag.to+shift;}else{state.time.from_ns=Math.min(drag.startValue,value);state.time.to_ns=Math.max(drag.startValue,value);if(state.time.from_ns===state.time.to_ns)state.time.to_ns=Math.min(bounds.to,state.time.from_ns+1);}updateWindow();}; svg.onpointerup=event=>{if(drag&&drag.pointerId===event.pointerId){drag=null;svg.releasePointerCapture(event.pointerId);render();}};svg.onpointercancel=svg.onpointerup;
    svg.onwheel=event=>{const value=timelineValue(event,svg,width,0,chartWidth),chartRect=chartScroll.getBoundingClientRect(),pointerX=event.clientX-chartRect.left,nextScale=Math.max(1,Math.min(8,timelineScale*Math.exp(-event.deltaY*0.002)));if(value<state.time.from_ns||value>state.time.to_ns||nextScale===timelineScale)return;event.preventDefault();timelineScale=nextScale;render();const nextWidth=Math.max(240,chartScroll.clientWidth*timelineScale);chartScroll.scrollLeft=Math.max(0,position(value)*nextWidth-pointerX);}; chartScroll.onscroll=()=>{if(labelScroll.scrollTop!==chartScroll.scrollTop)labelScroll.scrollTop=chartScroll.scrollTop;};labelScroll.onscroll=()=>{if(chartScroll.scrollTop!==labelScroll.scrollTop)chartScroll.scrollTop=labelScroll.scrollTop;};labelScroll.scrollTop=chartScroll.scrollTop;
  }
  function paintHistogramLegacy(result) {
    const all=result.all,svg=id('histogram'),width=Math.max(480,svg.clientWidth||720),height=126,bins=30,railHeight=20;svg.innerHTML='';svg.setAttribute('viewBox',`0 0 ${width} ${height}`);histogramView=null;if(!all.length)return;const durations=result.durations||all.map(invocation=>invocation.end_ns-invocation.start_ns),min=Math.min(...durations),max=Math.max(...durations),span=Math.max(1,max-min),counts=Array(bins).fill(0),bucketByIndex=[];durations.forEach((duration,index)=>{const bucket=Math.min(bins-1,Math.floor((duration-min)/span*bins));counts[bucket]++;bucketByIndex[index]=bucket;});const peak=Math.max(1,...counts),rail=svgEl('rect',{class:'histogram-rail',x:0,y:0,width,height:railHeight}),bars=[];svg.appendChild(rail);counts.forEach((value,index)=>{const x=index*width/bins,h=value/peak*76,bar=svgEl('rect',{class:'hist-bar',x:x+1,y:98-h,width:width/bins-2,height:h,rx:2});bars.push(bar);svg.appendChild(bar);});const xFor=value=>Math.max(0,Math.min(width,(value-min)/span*width)),railSelection=svgEl('rect',{class:'histogram-rail-selection',x:0,y:0,width:2,height:railHeight}),windowRect=svgEl('rect',{class:'hist-window',x:0,y:railHeight,width:2,height:103-railHeight}),lowLine=svgEl('line',{class:'hist-handle','data-edge':'low',y1:0,y2:102}),highLine=svgEl('line',{class:'hist-handle','data-edge':'high',y1:0,y2:102}),lowHit=svgEl('rect',{class:'hist-handle-hit','data-edge':'low',y:0,width:16,height:103}),highHit=svgEl('rect',{class:'hist-handle-hit','data-edge':'high',y:0,width:16,height:103});svg.appendChild(windowRect);svg.appendChild(railSelection);svg.appendChild(lowLine);svg.appendChild(highLine);svg.appendChild(lowHit);svg.appendChild(highHit);const label=(text,x,anchor='middle')=>{const node=svgEl('text',{class:'hist-label',x,y:119,'text-anchor':anchor});node.textContent=text;svg.appendChild(node);};const niceStep=span=>{const raw=span/5,power=10**Math.floor(Math.log10(raw)),scaled=raw/power,factor=scaled>=5?5:scaled>=2?2:1;return factor*power;};const ticks=[min],step=niceStep(span);for(let value=Math.ceil(min/step)*step;value<max;value+=step){if(value>min)ticks.push(value);}ticks.push(max);ticks.forEach((value,index)=>{const x=xFor(value);svg.appendChild(svgEl('line',{class:'hist-axis-tick',x1:x,x2:x,y1:102,y2:108}));label(ns(value),x,index===0?'start':index===ticks.length-1?'end':'middle');});
    const valueAt=(event,rect)=>min+Math.max(0,Math.min(1,(event.clientX-rect.left)/rect.width))*span; const update=()=>{const low=state.latency.low_ns,high=state.latency.high_ns,lowX=xFor(low),highX=xFor(high),range=rankBounds(all.length,state.percentile),selected=Array(bins).fill(false);for(let index=range[0];index<range[1];index++)selected[bucketByIndex[index]]=true;bars.forEach((bar,index)=>bar.setAttribute('class',selected[index]?'hist-selected':'hist-bar'));windowRect.setAttribute('x',lowX);windowRect.setAttribute('width',Math.max(2,highX-lowX));railSelection.setAttribute('x',lowX);railSelection.setAttribute('width',Math.max(2,highX-lowX));lowLine.setAttribute('x1',lowX);lowLine.setAttribute('x2',lowX);highLine.setAttribute('x1',highX);highLine.setAttribute('x2',highX);lowHit.setAttribute('x',lowX-8);highHit.setAttribute('x',highX-8);id('pct-low').value=state.percentile.low;id('pct-high').value=state.percentile.high;};histogramView={all,durations,min,max,span,svg,width,update};
    const begin=(kind,event)=>{const rect=svg.getBoundingClientRect(),next=valueAt(event,rect),edgeValue=kind==='low'?state.latency.low_ns:state.latency.high_ns;drag={kind:kind==='move'?'latency-move':kind==='select'?'latency-select':`latency-${kind}`,edge:kind,pointerId:event.pointerId,rect,startDuration:next,low:state.latency.low_ns,high:state.latency.high_ns,offset:next-state.latency.low_ns,edgeOffset:next-edgeValue};svg.setPointerCapture(event.pointerId);event.preventDefault();};lowHit.addEventListener('pointerdown',event=>{event.stopPropagation();begin('low',event);});highHit.addEventListener('pointerdown',event=>{event.stopPropagation();begin('high',event);});svg.onpointerdown=event=>{const rect=svg.getBoundingClientRect(),inRail=event.clientY-rect.top<railHeight;if(event.target.tagName!=='text'&&!event.target.classList.contains('hist-handle-hit'))begin(inRail?'move':'select',event);};svg.onpointermove=event=>{if(!drag||drag.pointerId!==event.pointerId||!drag.kind.startsWith('latency-'))return;const next=valueAt(event,drag.rect);if(drag.kind==='latency-low')state.latency.low_ns=Math.min(next-drag.edgeOffset,state.latency.high_ns);else if(drag.kind==='latency-high')state.latency.high_ns=Math.max(next-drag.edgeOffset,state.latency.low_ns);else if(drag.kind==='latency-move'){const size=drag.high-drag.low,low=Math.max(min,Math.min(max-size,next-drag.offset));state.latency.low_ns=low;state.latency.high_ns=low+size;}else{const low=Math.min(drag.startDuration,next),high=Math.max(drag.startDuration,next);state.latency.low_ns=low;state.latency.high_ns=high>low?high:Math.min(max,low+1);}syncLatencyWindow(durations);histogramView.update();};svg.onpointerup=event=>{if(drag&&drag.pointerId===event.pointerId){drag=null;svg.releasePointerCapture(event.pointerId);render();}};svg.onpointercancel=svg.onpointerup;update();
  }
  function paintHistogram(result) {
    const all = result.all;
    const svg = id('histogram');
    const width = Math.max(480, svg.clientWidth || 720);
    const height = 126;
    const railHeight = 20;
    svg.innerHTML = '';
    svg.setAttribute('viewBox', `0 0 ${width} ${height}`);
    histogramView = null;
    if (!all.length) return;

    const durations = result.durations || all.map(invocation => invocation.end_ns - invocation.start_ns);
    const rawMin = Math.min(...durations);
    const rawMax = Math.max(...durations);
    const bucketSize = state.histogramBucketSizeNs;
    const min = bucketSize ? Math.floor(rawMin / bucketSize) * bucketSize : rawMin;
    const bins = bucketSize ? Math.max(1, Math.ceil((rawMax - min) / bucketSize)) : 30;
    const max = bucketSize ? min + bins * bucketSize : rawMax;
    const span = Math.max(1, max - min);
    const counts = Array(bins).fill(0);
    const bucketByIndex = [];
    durations.forEach((duration, index) => {
      const bucket = Math.min(bins - 1, Math.floor((duration - min) / span * bins));
      counts[bucket]++;
      bucketByIndex[index] = bucket;
    });
    const peak = Math.max(1, ...counts);
    const rail = svgEl('rect', { class: 'histogram-rail', x: 0, y: 0, width, height: railHeight });
    const bars = [];
    svg.appendChild(rail);
    counts.forEach((value, index) => {
      const x = index * width / bins;
      const h = value ? value / peak * 76 : 2;
      const bar = svgEl('rect', { class: value ? 'hist-bar' : 'hist-bar hist-empty', x: x + 1, y: value ? 98 - h : 96, width: width / bins - 2, height: h, rx: 2 });
      const bucketLow = min + index * span / bins;
      const bucketHigh = min + (index + 1) * span / bins;
      const bucketLabel = `${ns(bucketLow)}–${ns(bucketHigh)} (upper bound exclusive)`;
      bar.setAttribute('data-frequency-range', bucketLabel);
      bar.setAttribute('data-sample-count', value);
      bar.setAttribute('aria-label', `Histogram bucket ${bucketLabel}; ${count(value)} samples (invocations)`);
      const title = svgEl('title');
      title.textContent = `Latency ${bucketLabel} · ${count(value)} samples (invocations)`;
      bar.appendChild(title);
      bar.addEventListener('mouseenter', event => showHistogramTooltip(event, bucketLow, bucketHigh, value));
      bar.addEventListener('mouseleave', hideTooltip);
      bars.push(bar);
      svg.appendChild(bar);
    });
    const xFor = value => Math.max(0, Math.min(width, (value - min) / span * width));
    const railSelection = svgEl('rect', { class: 'histogram-rail-selection', x: 0, y: 0, width: 2, height: railHeight });
    const windowRect = svgEl('rect', { class: 'hist-window', x: 0, y: railHeight, width: 2, height: 103 - railHeight });
    const lowLine = svgEl('line', { class: 'hist-handle', 'data-edge': 'low', y1: 0, y2: 102 });
    const highLine = svgEl('line', { class: 'hist-handle', 'data-edge': 'high', y1: 0, y2: 102 });
    const lowHit = svgEl('rect', { class: 'hist-handle-hit', 'data-edge': 'low', y: 0, width: 16, height: 103 });
    const highHit = svgEl('rect', { class: 'hist-handle-hit', 'data-edge': 'high', y: 0, width: 16, height: 103 });
    svg.appendChild(windowRect);
    svg.appendChild(railSelection);
    svg.appendChild(lowLine);
    svg.appendChild(highLine);
    svg.appendChild(lowHit);
    svg.appendChild(highHit);

    const label = (text, x, anchor = 'middle') => {
      const node = svgEl('text', { class: 'hist-label', x, y: 119, 'text-anchor': anchor });
      node.textContent = text;
      svg.appendChild(node);
    };
    const niceStep = value => {
      const raw = value / 5;
      const power = 10 ** Math.floor(Math.log10(raw));
      const scaled = raw / power;
      return (scaled >= 5 ? 5 : scaled >= 2 ? 2 : 1) * power;
    };
    const ticks = [min];
    const step = niceStep(span);
    for (let value = Math.ceil(min / step) * step; value < max; value += step) {
      if (value > min) ticks.push(value);
    }
    ticks.push(max);
    ticks.forEach((value, index) => {
      const x = xFor(value);
      svg.appendChild(svgEl('line', { class: 'hist-axis-tick', x1: x, x2: x, y1: 102, y2: 108 }));
      label(ns(value), x, index === 0 ? 'start' : index === ticks.length - 1 ? 'end' : 'middle');
    });

    const valueAt = (event, rect) => min + Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)) * span;
    const update = () => {
      const low = state.latency.low_ns;
      const high = state.latency.high_ns;
      const lowX = xFor(low);
      const highX = xFor(high);
      const range = rankBounds(all.length, state.percentile);
      const selected = Array(bins).fill(false);
      for (let index = range[0]; index < range[1]; index++) selected[bucketByIndex[index]] = true;
      bars.forEach((bar, index) => bar.setAttribute('class', selected[index] ? 'hist-selected' : 'hist-bar'));
      windowRect.setAttribute('x', lowX);
      windowRect.setAttribute('width', Math.max(2, highX - lowX));
      railSelection.setAttribute('x', lowX);
      railSelection.setAttribute('width', Math.max(2, highX - lowX));
      lowLine.setAttribute('x1', lowX);
      lowLine.setAttribute('x2', lowX);
      highLine.setAttribute('x1', highX);
      highLine.setAttribute('x2', highX);
      lowHit.setAttribute('x', lowX - 8);
      highHit.setAttribute('x', highX - 8);
      id('pct-low').value = state.percentile.low;
      id('pct-high').value = state.percentile.high;
    };
    histogramView = { all, durations, min, max, span, svg, width, update };

    const begin = (kind, event) => {
      const rect = svg.getBoundingClientRect();
      const next = valueAt(event, rect);
      const edgeValue = kind === 'low' ? state.latency.low_ns : state.latency.high_ns;
      drag = { kind: kind === 'move' ? 'latency-move' : kind === 'select' ? 'latency-select' : `latency-${kind}`, edge: kind, pointerId: event.pointerId, rect, startDuration: next, low: state.latency.low_ns, high: state.latency.high_ns, offset: next - state.latency.low_ns, edgeOffset: next - edgeValue };
      svg.setPointerCapture(event.pointerId);
      event.preventDefault();
    };
    lowHit.addEventListener('pointerdown', event => { event.stopPropagation(); begin('low', event); });
    highHit.addEventListener('pointerdown', event => { event.stopPropagation(); begin('high', event); });
    svg.onpointerdown = event => {
      const rect = svg.getBoundingClientRect();
      const inRail = event.clientY - rect.top < railHeight;
      if (event.target.tagName !== 'text' && !event.target.classList.contains('hist-handle-hit')) begin(inRail ? 'move' : 'select', event);
    };
    svg.onpointermove = event => {
      if (!drag || drag.pointerId !== event.pointerId || !drag.kind.startsWith('latency-')) return;
      const next = valueAt(event, drag.rect);
      if (drag.kind === 'latency-low') state.latency.low_ns = Math.min(next - drag.edgeOffset, state.latency.high_ns);
      else if (drag.kind === 'latency-high') state.latency.high_ns = Math.max(next - drag.edgeOffset, state.latency.low_ns);
      else if (drag.kind === 'latency-move') {
        const size = drag.high - drag.low;
        const low = Math.max(min, Math.min(max - size, next - drag.offset));
        state.latency.low_ns = low;
        state.latency.high_ns = low + size;
      } else {
        const low = Math.min(drag.startDuration, next);
        const high = Math.max(drag.startDuration, next);
        state.latency.low_ns = low;
        state.latency.high_ns = high > low ? high : Math.min(max, low + 1);
      }
      syncLatencyWindow(durations);
      histogramView.update();
    };
    svg.onpointerup = event => { if (drag && drag.pointerId === event.pointerId) { drag = null; svg.releasePointerCapture(event.pointerId); render(); } };
    svg.onpointercancel = svg.onpointerup;
    update();
  }
  function flameColor(name) { let hash=0; for(let index=0;index<name.length;index++) hash=(hash*33+name.charCodeAt(index))>>>0; return `hsl(${20+hash%46} ${64+hash%19}% ${48+hash%17}%)`; }
  const nodeAtPath=(root,path)=>path.reduce((current,name)=>current && current.children.get(name),root);
  /* Superseded flame layout implementation retained only in source history.
  function renderFlame(root) { const svg=id('flame'); svg.innerHTML=''; let source=nodeAtPath(root,zoomPath); if(!source){zoomPath=[]; source=root;} if(!root.value) { svg.hidden=true; return; } svg.hidden=false; const width=Math.max(480,svg.clientWidth||1000), row=25, items=[], basePath=zoomPath.slice(); const walk=(current,x,w,path,depth)=>{items.push({current,x,w,path,depth}); let cursor=x; [...current.children.values()].filter(child=>child.value>0).sort((a,b)=>b.value-a.value||a.name.localeCompare(b.name)).forEach(child=>{const childWidth=w*(child.value/current.value); walk(child,cursor,childWidth,path.concat(child.name),depth+1); cursor+=childWidth;});}; walk(source,0,width,basePath,0); const maxDepth=Math.max(...items.map(item=>item.depth)), height=Math.max(130,(maxDepth+1)*row+5); svg.setAttribute('viewBox',`0 0 ${width} ${height}`); items.forEach(item=>{ if(item.current===source && source.name==='root') return; const y=(maxDepth-item.depth)*row, group=svgEl('g'), match=state.search && item.current.name.toLowerCase().includes(state.search); const rect=svgEl('rect',{class:`frame${match?' match':''}`,x:item.x,y,width:Math.max(0,item.w),height:row-2,fill:flameColor(item.current.name),rx:2}); const title=svgEl('title'); title.textContent=`${item.current.name} — ${ns(item.current.value)}, ${item.current.sampleCount} samples`; group.appendChild(title); group.appendChild(rect); if(item.w>74){const label=svgEl('text',{class:'frame-label',x:item.x+5,y:y+(row-2)/2}); label.textContent=item.current.name.slice(0,Math.max(8,Math.floor(item.w/7))); group.appendChild(label);} rect.addEventListener('mouseenter',event=>showFrameTooltip(event,item.current)); rect.addEventListener('mouseleave',hideTooltip); rect.addEventListener('click',()=>{zoomPath=item.path; render();}); svg.appendChild(group); }); }
  */
  function placeTooltip(event) { const tooltip=id('flame-tooltip'), margin=12, gap=14, box=tooltip.getBoundingClientRect(), maxLeft=Math.max(margin,window.innerWidth-box.width-margin), maxTop=Math.max(margin,window.innerHeight-box.height-margin); let left=event.clientX+gap, top=event.clientY+gap; if(left+box.width>window.innerWidth-margin) left=event.clientX-gap-box.width; if(top+box.height>window.innerHeight-margin) top=event.clientY-gap-box.height; tooltip.style.left=`${Math.max(margin,Math.min(maxLeft,left))}px`; tooltip.style.top=`${Math.max(margin,Math.min(maxTop,top))}px`; }
  function showFrameTooltip(event,current) { const tooltip=id('flame-tooltip'); tooltip.innerHTML=`<strong>${escapeHtml(current.name)}</strong><span>Inclusive: ${ns(current.value)} · Self: ${ns(current.selfValue)}</span><span>Samples: ${count(current.sampleCount)} · Invocations: ${count(current.invocations.size)}</span><span>CPU: ${ns(current.cpu)} · Off-CPU: ${ns(current.off)}</span>`; tooltip.hidden=false; placeTooltip(event); }
  function showHistogramTooltip(event, low, high, samples) { const tooltip=id('flame-tooltip'); tooltip.innerHTML=`<strong>Histogram bucket</strong><span>Latency: ${ns(low)}–${ns(high)} (upper bound exclusive)</span><span>Samples (invocations): ${count(samples)}</span>`; tooltip.hidden=false; placeTooltip(event); }
  function showTimelineTooltip(event,invocation) { const tooltip=id('flame-tooltip'); tooltip.innerHTML=`<strong>Invocation #${invocation.id}</strong><span>TID ${invocation.tid} · ${ns(invocation.end_ns-invocation.start_ns)} wall</span><span>Start ${ns(invocation.start_ns-bounds.from)} · End ${ns(invocation.end_ns-bounds.from)}</span>`; tooltip.hidden=false; placeTooltip(event); }
  function hideTooltip() { id('flame-tooltip').hidden=true; }
  function renderZoomPath() { const nav=id('flame-zoom-path'); if(!nav)return; nav.innerHTML=''; const add=(label,path,index)=>{if(index) {const separator=document.createElement('span');separator.className='flame-zoom-separator';separator.textContent='›';nav.appendChild(separator);} const button=document.createElement('button');button.type='button';button.textContent=label;button.title=label;if(index===zoomPath.length)button.setAttribute('aria-current','page');button.addEventListener('click',()=>{zoomPath=path;render();});nav.appendChild(button);}; add('All stacks',[],0); zoomPath.forEach((name,index)=>add(name,zoomPath.slice(0,index+1),index+1)); }
  function renderFlame(root) { const svg=id('flame'); svg.innerHTML=''; renderZoomPath(); let source=nodeAtPath(root,zoomPath); if(!source){zoomPath=[];source=root;renderZoomPath();} if(!root.value) { svg.hidden=true; return; } svg.hidden=false; const width=Math.max(480,svg.clientWidth||1000),row=25,items=[],walk=(current,x,w,path,depth)=>{items.push({current,x,w,path,depth});let cursor=x;[...current.children.values()].filter(child=>child.value>0).sort((a,b)=>b.value-a.value||a.name.localeCompare(b.name)).forEach(child=>{const childWidth=w*(child.value/current.value);walk(child,cursor,childWidth,path.concat(child.name),depth+1);cursor+=childWidth;});}; if(source.name==='root'){let cursor=0;[...source.children.values()].filter(child=>child.value>0).sort((a,b)=>b.value-a.value||a.name.localeCompare(b.name)).forEach(child=>{const childWidth=width*(child.value/source.value);walk(child,cursor,childWidth,[child.name],0);cursor+=childWidth;});}else walk(source,0,width,zoomPath,0); const maxDepth=Math.max(0,...items.map(item=>item.depth)),rowHeight=25,height=Math.max(160,(maxDepth+1)*rowHeight+12); svg.style.height=`${height}px`; svg.setAttribute('viewBox',`0 0 ${width} ${height}`); items.forEach(item=>{const y=(maxDepth-item.depth)*rowHeight,group=svgEl('g'),match=state.search&&item.current.name.toLowerCase().includes(state.search),rect=svgEl('rect',{class:`frame${match?' match':''}`,x:item.x,y,width:Math.max(0,item.w),height:rowHeight-2,fill:flameColor(item.current.name),rx:2});const title=svgEl('title');title.textContent=`${item.current.name} — ${ns(item.current.value)}, ${item.current.sampleCount} samples`;group.appendChild(title);group.appendChild(rect);if(item.w>74){const label=svgEl('text',{class:'frame-label',x:item.x+5,y:y+(rowHeight-2)/2});label.textContent=item.current.name.slice(0,Math.max(8,Math.floor(item.w/7)));group.appendChild(label);}rect.addEventListener('mouseenter',event=>showFrameTooltip(event,item.current));rect.addEventListener('mouseleave',hideTooltip);rect.addEventListener('click',()=>{zoomPath=item.path;render();});svg.appendChild(group);}); }
  function render() { const result=calculate(), fn=byFunction.get(state.functionId); id('population-name').textContent=fn ? fn.demangled_name : 'Unknown function'; id('population-detail').textContent=`${count(result.all.length)} valid invocations after thread/time filters`; id('capture-command').textContent=profile.metadata.command.length ? profile.metadata.command.join(' ') : 'unknown command'; id('capture-range').textContent=`capture ${ns(bounds.to-bounds.from)}`; id('selected-count').textContent=count(result.chosen.length); id('latency-range').textContent=result.chosen.length?`${ns(Math.min(...result.chosen.map(invocation=>invocation.end_ns-invocation.start_ns)))} – ${ns(Math.max(...result.chosen.map(invocation=>invocation.end_ns-invocation.start_ns)))}`:'no selected latency'; id('sample-count').textContent=count(result.selectedSampleCount); id('sample-period').textContent=`period ${ns(profile.metadata.sample_period_ns)}`; id('cpu-time').textContent=ns(result.cpu); id('offcpu-time').textContent=`off-CPU ${ns(result.off)}`; id('time-low').value=relativeMs(state.time.from_ns); id('time-high').value=relativeMs(state.time.to_ns); id('time-low').max=relativeMs(bounds.to); id('time-high').max=relativeMs(bounds.to); id('pct-low').value=state.percentile.low; id('pct-high').value=state.percentile.high; id('thread-summary').textContent=state.threads.size===allThreadIds.length?'All observed threads':state.threads.size?`${state.threads.size} of ${allThreadIds.length} threads`:'No threads selected'; id('threads').querySelectorAll('input').forEach(input=>{input.checked=state.threads.has(Number(input.value));}); id('empty').hidden=Boolean(result.root.value); paintTimeline(result); paintHistogram(result); renderFlame(result.root); }
  function decorateFlame() { id('flame').querySelectorAll('.frame').forEach(frame=>{if(frame.getAttribute('data-slice-a11y'))return;const title=frame.querySelector('title'),label=title ? title.textContent : 'Flame graph frame';frame.setAttribute('tabindex','0');frame.setAttribute('role','button');frame.setAttribute('aria-label',label);frame.setAttribute('aria-keyshortcuts','Enter Space');frame.setAttribute('data-slice-a11y','true');frame.addEventListener('keydown',event=>{if(event.key==='Enter'||event.key===' '){event.preventDefault();frame.click();}});}); }
  function decorateTimeline() { id('timeline').querySelectorAll('.timeline-invocation').forEach(bar=>{if(bar.getAttribute('data-slice-a11y'))return;bar.setAttribute('tabindex','0');bar.setAttribute('role','img');bar.setAttribute('aria-label','Invocation timeline bar; hover for timing details');bar.setAttribute('data-slice-a11y','true');}); }
  const observers = [['flame',decorateFlame],['timeline',decorateTimeline]]; observers.forEach(([name,decorate])=>{if(typeof MutationObserver==='function'){new MutationObserver(decorate).observe(id(name),{childList:true});}});
  function bind() {
    const threads=id('threads'); threadRows.forEach(thread=>{const label=document.createElement('label'), input=document.createElement('input'); input.type='checkbox'; input.value=thread.tid; input.checked=state.threads.has(thread.tid); const text=document.createTextNode(`${thread.name || 'TID'} ${thread.tid}`); label.append(input,text); threads.appendChild(label);}); threads.addEventListener('change',event=>{if(event.target.matches('input')){const tid=Number(event.target.value); event.target.checked?state.threads.add(tid):state.threads.delete(tid); render();}});
    const commitTime=(element,key)=>{const entered=Number(element.value); if(!Number.isFinite(entered)) return render(); const value=clampTime(bounds.from+entered*1e6); if(key==='from_ns') state.time.from_ns=Math.min(value,state.time.to_ns-1); else state.time.to_ns=Math.max(value,state.time.from_ns+1); render();}; id('time-low').addEventListener('change',event=>commitTime(event.target,'from_ns')); id('time-high').addEventListener('change',event=>commitTime(event.target,'to_ns'));
    const commitPercentile=(element,key)=>{const entered=Number(element.value), durations=eligible().map(invocation=>invocation.end_ns-invocation.start_ns); if(!Number.isFinite(entered)||!durations.length) return render(); const next=key==='low'?clampPercentileWindow(entered,state.percentile.high):clampPercentileWindow(state.percentile.low,entered); state.percentile=next; state.latency={low_ns:durationAtPercentile(durations,next.low),high_ns:durationAtPercentile(durations,next.high)}; render();}; id('pct-low').addEventListener('change',event=>commitPercentile(event.target,'low')); id('pct-high').addEventListener('change',event=>commitPercentile(event.target,'high'));
    id('histogram-bucket-size').value=state.histogramBucketSizeNs ?? 'auto'; id('histogram-bucket-size').addEventListener('change',event=>{state.histogramBucketSizeNs=event.target.value==='auto'?null:Number(event.target.value); render();}); id('metric').value=state.metric; id('metric').addEventListener('change',event=>{state.metric=event.target.value; render();}); id('frame-search').addEventListener('input',event=>{state.search=event.target.value.trim().toLowerCase(); renderFlame(calculate().root);});
  }
  bind(); render(); decorateFlame(); decorateTimeline(); window.addEventListener('resize',()=>render());
  } catch (error) {
    const failure = document.getElementById('viewer-error');
    if (failure) {
      failure.hidden = false;
      failure.textContent = `Viewer failed to load: ${error instanceof Error ? error.message : String(error)}`;
    }
    console.error('Slice viewer failed to load', error);
  }
})();
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use slice_core::{
        CaptureQuality, ExecutionState, Frame, Function, Invocation, Metadata, Sample, Stack,
        Thread,
    };

    fn profile() -> Profile {
        Profile {
            format_version: 1,
            metadata: Metadata {
                captured_at_unix_ns: 1,
                command: vec!["test-program".to_owned()],
                kernel_release: "test-kernel".to_owned(),
                sample_period_ns: 1_000_000,
            },
            functions: vec![Function {
                id: 1,
                module: "test-module".to_owned(),
                module_build_id: Some("test-build".to_owned()),
                address: 0x1000,
                name: "work".to_owned(),
                demangled_name: "BimodalFixture::handle_request(unsigned long)".to_owned(),
                source_file: None,
                line: None,
            }],
            threads: vec![Thread {
                tid: 10,
                name: Some("worker".to_owned()),
            }],
            invocations: vec![Invocation {
                id: 1,
                function_id: 1,
                parent_id: None,
                tid: 10,
                start_ns: 0,
                end_ns: 20_000_000,
                complete: true,
                valid: true,
            }],
            stacks: vec![Stack {
                id: 1,
                frames: vec![
                    Frame {
                        function_id: Some(1),
                        label: "BimodalFixture::handle_request(unsigned long)".to_owned(),
                        module: Some("test-module".to_owned()),
                        address: Some(0x1000),
                    },
                    Frame {
                        function_id: None,
                        label: "BimodalFixture::slow_path()".to_owned(),
                        module: Some("test-module".to_owned()),
                        address: None,
                    },
                ],
            }],
            samples: vec![Sample {
                timestamp_ns: 10_000_000,
                invocation_id: 1,
                stack_id: 1,
                tid: 10,
                cpu: 0,
                state: ExecutionState::OnCpu,
                weight_ns: 20_000_000,
            }],
            quality: CaptureQuality {
                events_generated: 2,
                samples_generated: 1,
                complete_invocations: 1,
                ..CaptureQuality::default()
            },
        }
    }

    #[test]
    fn report_is_one_offline_file_with_timeline_and_frame_details() {
        let profile = profile();
        let html = render_html(
            &profile,
            &Query {
                function_id: 1,
                threads: None,
                time: None,
                percentile: PercentileRange::ALL,
                metric: Metric::Wall,
            },
        )
        .unwrap();
        assert!(html.contains("BimodalFixture::slow_path()"));
        assert!(html.contains("id=\"timeline\""));
        assert!(html.contains("id=\"timeline-scroll\""));
        assert!(html.contains("id=\"timeline-labels-scroll\""));
        assert!(html.contains("id=\"timeline-chart-scroll\""));
        assert!(html.contains("id=\"flame-tooltip\""));
        assert!(html.contains("callers above the population selector are intentionally omitted"));
        assert!(html.contains("id=\"viewer-error\""));
        assert!(html.contains("new MutationObserver"));
        assert!(html.contains("aria-keyshortcuts"));
        assert!(html.contains("Samples:"));
        assert!(html.contains("id=\"time-low\""));
        assert!(html.contains("id=\"pct-low\""));
        assert!(html.contains("preserveAspectRatio=\"none\""));
        assert!(html.contains("id=\"thread-picker\""));
        assert!(html.contains("drag the capture-start strip to move it"));
        assert!(html.contains("stack.frames.slice(startFrame)"));
        assert!(html.contains("const maxDepth"));
        assert!(html.contains("time-handle-hit"));
        assert!(html.contains("time-select"));
        assert!(html.contains("latency-select"));
        assert!(html.contains("histogram-rail"));
        assert!(html.contains("hist-axis-tick"));
        assert!(html.contains("niceStep"));
        assert!(html.contains("inRail"));
        assert!(html.contains("time-move"));
        assert!(html.contains("stroke-width:4"));
        assert!(html.contains("box-shadow:8px 8px 0 var(--ink)"));
        assert!(html.contains("hist-handle-hit"));
        assert!(html.contains("startDuration"));
        assert!(html.contains("syncLatencyWindow(durations)"));
        assert!(html.contains("histogramView.update()"));
        assert!(html.contains("histogram-bucket-size"));
        assert!(html.contains("histogramBucketSizeNs"));
        assert!(html.contains("const bucketSize = state.histogramBucketSizeNs"));
        assert!(html.contains("data-frequency-range"));
        assert!(html.contains("showHistogramTooltip"));
        assert!(html.contains("startValue:timelineValue"));
        assert!(html.contains("drag.rect"));
        assert!(!html.contains("startPercentile"));
        assert!(html.contains("svg.onwheel"));
        assert!(html.contains("timelineScale"));
        assert!(html.contains("overflow-x:scroll"));
        assert!(html.contains("id=\"flame-zoom-path\""));
        assert!(html.contains("renderZoomPath"));
        assert!(html.contains("placeTooltip"));
        assert!(!html.contains("id=\"reset-zoom\""));
        assert!(html.contains("y=(maxDepth-item.depth)*row"));
        assert!(html.contains(": p"));
        assert!(!html.contains("type=\"range\""));
        assert!(!html.contains("https://"));
        assert!(!html.contains("fetch("));
    }

    #[test]
    fn renderer_rejects_invalid_profiles_before_creating_a_report() {
        let mut profile = profile();
        profile.samples[0].stack_id = 999;
        let error = render_html(
            &profile,
            &Query {
                function_id: 1,
                threads: None,
                time: None,
                percentile: PercentileRange { low: 95, high: 100 },
                metric: Metric::Wall,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            RenderError::InvalidProfile(ProfileValidationError::UnknownStack(999))
        ));
    }
}
