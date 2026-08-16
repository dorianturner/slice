//! Self-contained HTML report generation.
//!
//! The browser implementation mirrors the small, pure query contract in
//! `slice-core` so an artifact can be opened with `file://` and needs neither a
//! server nor a network connection.

use serde::Serialize;
use slice_core::{Metric, PercentileRange, Profile, Query, TimeRange};

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

/// Return a complete offline HTML viewer.  The file intentionally includes no
/// external URLs, fonts, scripts, or XHR/fetch calls.
pub fn render_html(profile: &Profile, query: &Query) -> Result<String, serde_json::Error> {
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
  <header>
    <div><p class="eyebrow">PERCENTILE-CONDITIONED PROFILE</p><h1>Slice</h1></div>
    <div id="quality" class="quality"></div>
  </header>
  <section class="population" aria-label="Population">
    <p class="label">Population</p><h2 id="population-name"></h2><p id="population-detail" class="muted"></p>
  </section>
  <section class="controls" aria-label="Profile controls">
    <fieldset><legend>Threads</legend><div id="threads" class="thread-list"></div></fieldset>
    <fieldset><legend>Time range <span id="time-label" class="value"></span></legend><div class="range-pair"><input id="time-low" type="range"><input id="time-high" type="range"></div></fieldset>
    <fieldset><legend>Invocation latency <span id="percentile-label" class="value"></span></legend><svg id="histogram" viewBox="0 0 640 108" role="img" aria-label="Invocation latency histogram"></svg><div class="range-pair"><input id="pct-low" type="range" min="0" max="100" step="1"><input id="pct-high" type="range" min="0" max="100" step="1"></div></fieldset>
    <fieldset><legend>Metric</legend><select id="metric"><option value="wall">Wall time</option><option value="cpu">CPU time</option><option value="off_cpu">Off-CPU time</option></select></fieldset>
  </section>
  <section class="summary" aria-live="polite"><div><p>Selected invocations</p><strong id="selected-count"></strong><span id="latency-range"></span></div><div><p>Sampled CPU time</p><strong id="cpu-time"></strong></div><div><p>Off-CPU time</p><strong id="offcpu-time"></strong></div></section>
  <section class="flame-section"><div class="section-heading"><div><p class="label">Selected execution paths</p><h2>Flame graph</h2></div><button id="reset-zoom" type="button">Reset zoom</button></div><p id="flame-hint" class="muted">Click a frame to zoom. Width represents the selected metric, not invocation latency.</p><svg id="flame" role="img" aria-label="Interactive flame graph"></svg><div id="empty" hidden>No samples match this query.</div></section>
</main>
<script id="slice-profile" type="application/json">{}</script>
<script id="slice-initial-query" type="application/json">{}</script>
<script>{}</script>
</body></html>"#,
        CSS, profile_json, initial_json, JAVASCRIPT
    ))
}

fn script_safe_json(value: &impl Serialize) -> Result<String, serde_json::Error> {
    serde_json::to_string(value).map(|json| {
        json.replace('&', "\\u0026")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
    })
}

const CSS: &str = r##"
:root { color-scheme: dark; font-family: Inter, ui-sans-serif, system-ui, sans-serif; background:#111318; color:#edf0f6; }
* { box-sizing: border-box; } body { margin:0; background:radial-gradient(circle at 10% -20%,#263041 0,#111318 42%); }
main { max-width:1280px; margin:auto; padding:28px clamp(16px,4vw,48px) 64px; } header { display:flex; align-items:end; justify-content:space-between; border-bottom:1px solid #2b303b; padding-bottom:20px; }
h1,h2,p { margin:0; } h1 { font-size:2.25rem; letter-spacing:-.04em; } h2 { font-size:1.15rem; } .eyebrow,.label { color:#98a4ba; font-size:.71rem; font-weight:750; letter-spacing:.12em; } .muted { color:#aeb7c8; font-size:.86rem; }
.quality { color:#b8f4c8; font-size:.82rem; text-align:right; } .population { padding:26px 0 18px; } .population h2 { margin-top:5px; font-size:1.35rem; } .population .muted { margin-top:5px; }
.controls { display:grid; grid-template-columns:1.25fr 2fr 3fr 1fr; gap:12px; } fieldset { min-width:0; margin:0; border:1px solid #2d3441; border-radius:10px; background:#171a21; padding:13px; } legend { color:#c9d2e3; font-size:.8rem; padding:0 5px; } .value { color:#8ac5ff; font-variant-numeric:tabular-nums; }
.thread-list { display:flex; flex-wrap:wrap; gap:7px; max-height:64px; overflow:auto; } .thread-list label { border:1px solid #394253; border-radius:5px; padding:3px 7px; font-size:.75rem; white-space:nowrap; } input,select,button { accent-color:#68b7ff; } select { background:#232937; border:1px solid #4a566d; border-radius:5px; color:#edf0f6; padding:5px; width:100%; }
.range-pair { display:grid; gap:3px; } input[type=range] { width:100%; margin:0; } #histogram { width:100%; height:72px; display:block; margin-bottom:4px; } .hist-bar { fill:#527fb0; } .hist-selected { fill:#82c4ff; }
.summary { display:grid; grid-template-columns:repeat(3,1fr); gap:12px; margin:20px 0; } .summary>div { background:#1a1f29; border-left:3px solid #6bbaf7; border-radius:5px; padding:12px 15px; } .summary p { color:#aeb7c8; font-size:.77rem; } .summary strong { font-size:1.25rem; display:block; margin-top:4px; font-variant-numeric:tabular-nums; } .summary span { font-size:.75rem; color:#aeb7c8; }
.flame-section { border:1px solid #2d3441; border-radius:12px; background:#171a21; padding:18px; } .section-heading { display:flex; justify-content:space-between; align-items:center; margin-bottom:7px; } button { color:#d8e7ff; background:#28354a; border:1px solid #47617e; padding:6px 9px; border-radius:5px; cursor:pointer; } button:hover { background:#354a67; }
#flame { width:100%; min-height:120px; display:block; margin-top:12px; background:#101318; border-radius:6px; } .frame { stroke:#111318; stroke-width:1; cursor:pointer; } .frame:hover { stroke:#fff; stroke-width:2; } .frame-label { fill:#151515; pointer-events:none; font-size:11px; dominant-baseline:middle; } #empty { color:#f5c28a; padding:32px; text-align:center; }
@media (max-width:850px) { .controls { grid-template-columns:1fr 1fr; } .controls fieldset:nth-child(3) { grid-column:span 2; } } @media (max-width:550px) { header { align-items:start; gap:12px; flex-direction:column; } .controls,.summary { grid-template-columns:1fr; } .controls fieldset:nth-child(3) { grid-column:auto; } }
"##;

const JAVASCRIPT: &str = r##"
(() => {
  'use strict';
  const profile = JSON.parse(document.getElementById('slice-profile').textContent);
  const initial = JSON.parse(document.getElementById('slice-initial-query').textContent);
  const byStack = new Map(profile.stacks.map(s => [s.id, s]));
  const byFunction = new Map(profile.functions.map(f => [f.id, f]));
  const bounds = profile.invocations.reduce((a, i) => ({from:Math.min(a.from,i.start_ns),to:Math.max(a.to,i.end_ns)}), {from:Number.MAX_SAFE_INTEGER,to:0});
  let zoom = null;
  const state = { functionId:initial.function_id, threads:new Set(initial.threads || profile.threads.map(t => t.tid)), time:initial.time || {from_ns:bounds.from,to_ns:bounds.to + 1}, percentile:initial.percentile, metric:initial.metric };
  const ns = value => `${(value / 1e6).toFixed(value >= 1e9 ? 0 : 2)} ms`;
  const pct = value => `${value.toFixed(0)}%`;
  const id = name => document.getElementById(name);
  const q = selector => document.querySelector(selector);
  function eligible() { return profile.invocations.filter(i => i.function_id===state.functionId && i.complete && i.valid && state.threads.has(i.tid) && i.start_ns>=state.time.from_ns && i.start_ns<state.time.to_ns).sort((a,b)=>(a.end_ns-a.start_ns)-(b.end_ns-b.start_ns) || a.id-b.id); }
  function quantile(sorted, p) { if (!sorted.length) return null; if (sorted.length===1) return sorted[0]; const r=(p/100)*(sorted.length-1), lo=Math.floor(r), hi=Math.ceil(r); return sorted[lo]+(sorted[hi]-sorted[lo])*(r-lo); }
  function calculate() {
    const all=eligible(), start=Math.min(all.length,Math.ceil(all.length*state.percentile.low/100)), end=Math.min(all.length,Math.ceil(all.length*state.percentile.high/100));
    const chosen=all.slice(start,end), ids=new Set(chosen.map(i=>i.id)), root={name:'root',value:0,children:new Map()}, totals={cpu:0,off:0};
    for (const sample of profile.samples) { if (!ids.has(sample.invocation_id)) continue; if (state.metric!=='wall' && state.metric!==sample.state) continue; const stack=byStack.get(sample.stack_id); if (!stack) continue; let node=root; node.value+=sample.weight_ns; for (const frame of stack.frames) { if (!node.children.has(frame.label)) node.children.set(frame.label,{name:frame.label,value:0,children:new Map()}); node=node.children.get(frame.label); node.value+=sample.weight_ns; } if(sample.state==='on_cpu') totals.cpu+=sample.weight_ns; else totals.off+=sample.weight_ns; }
    return {all,chosen,root,totals,low:quantile(all.map(i=>i.end_ns-i.start_ns),state.percentile.low),high:quantile(all.map(i=>i.end_ns-i.start_ns),state.percentile.high)};
  }
  function paintHistogram(all) { const svg=id('histogram'), durations=all.map(i=>i.end_ns-i.start_ns), width=640, height=108, bins=24; svg.innerHTML=''; if(!durations.length) return; const max=Math.max(...durations), min=Math.min(...durations), span=Math.max(1,max-min), counts=Array(bins).fill(0); durations.forEach(v=>counts[Math.min(bins-1,Math.floor((v-min)/span*bins))]++); const peak=Math.max(...counts); counts.forEach((count,index)=>{ const x=index*width/bins, h=count/peak*94, center=(index+.5)/bins*100; const selected=center>=state.percentile.low && center<=state.percentile.high; svg.insertAdjacentHTML('beforeend',`<rect class="${selected?'hist-selected':'hist-bar'}" x="${x+1}" y="${100-h}" width="${width/bins-2}" height="${h}" rx="2"/>`); }); svg.insertAdjacentHTML('beforeend',`<text fill="#aeb7c8" x="0" y="107" font-size="10">${ns(min)}</text><text fill="#aeb7c8" x="${width-62}" y="107" font-size="10">${ns(max)}</text>`); }
  function color(name) { let h=0; for(let i=0;i<name.length;i++) h=(h*33+name.charCodeAt(i))>>>0; return `hsl(${20+h%46} ${64+h%19}% ${48+h%17}%)`; }
  function renderFlame(root) { const svg=id('flame'), source=zoom || root; svg.innerHTML=''; const width=Math.max(400,svg.clientWidth || 900), row=25, nodes=[]; const walk=(node,x,y,w)=>{nodes.push({node,x,y,w}); let cursor=x; for(const child of [...node.children.values()].sort((a,b)=>b.value-a.value||a.name.localeCompare(b.name))) { const childWidth=w*(child.value/node.value); walk(child,cursor,y+row,childWidth); cursor+=childWidth; }}; if(source.value) walk(source,0,0,width); svg.setAttribute('viewBox',`0 0 ${width} ${Math.max(120,(Math.max(0,...nodes.map(n=>n.y))+row+4))}`); for(const item of nodes) { if(item.node===source && source.name==='root') continue; const label=item.node.name.replace(/[&<>]/g,''); svg.insertAdjacentHTML('beforeend',`<g><title>${label} — ${ns(item.node.value)}</title><rect class="frame" data-name="${encodeURIComponent(item.node.name)}" x="${item.x}" y="${item.y}" width="${Math.max(0,item.w)}" height="${row-2}" fill="${color(item.node.name)}" rx="2"/><text class="frame-label" x="${item.x+5}" y="${item.y+(row-2)/2}">${item.w>85?label.slice(0,Math.max(8,Math.floor(item.w/7))):''}</text></g>`); } svg.querySelectorAll('.frame').forEach(rect=>rect.addEventListener('click',()=>{ const wanted=decodeURIComponent(rect.dataset.name); const find=node=>node.name===wanted?node:[...node.children.values()].map(find).find(Boolean); zoom=find(root); renderFlame(root); })); }
  function render() { const result=calculate(), fn=byFunction.get(state.functionId); id('population-name').textContent=fn ? fn.demangled_name : 'Unknown function'; id('population-detail').textContent=`${result.all.length} valid invocations after thread/time filters`; id('selected-count').textContent=result.chosen.length; id('latency-range').textContent=result.chosen.length?`${ns(Math.min(...result.chosen.map(i=>i.end_ns-i.start_ns)))} – ${ns(Math.max(...result.chosen.map(i=>i.end_ns-i.start_ns)))}`:'no selected latency'; id('cpu-time').textContent=ns(result.totals.cpu); id('offcpu-time').textContent=ns(result.totals.off); id('percentile-label').textContent=`p${state.percentile.low}:p${state.percentile.high}`; id('time-label').textContent=`${ns(state.time.from_ns-bounds.from)} – ${ns(state.time.to_ns-bounds.from)}`; id('empty').hidden=Boolean(result.root.value); id('flame').hidden=!result.root.value; paintHistogram(result.all); renderFlame(result.root); }
  function bind() { const threads=id('threads'); profile.threads.forEach(thread=>{ const key=`thread-${thread.tid}`, checked=state.threads.has(thread.tid)?'checked':''; threads.insertAdjacentHTML('beforeend',`<label for="${key}"><input id="${key}" type="checkbox" value="${thread.tid}" ${checked}> ${thread.name || 'TID'} ${thread.tid}</label>`); }); threads.addEventListener('change',event=>{const tid=Number(event.target.value); event.target.checked?state.threads.add(tid):state.threads.delete(tid); zoom=null; render();});
    const configureTime=(element,key)=>{element.min=bounds.from;element.max=bounds.to+1;element.step=Math.max(1,Math.round((bounds.to-bounds.from)/600));element.value=state.time[key];element.addEventListener('input',()=>{state.time[key]=Number(element.value);if(state.time.from_ns>=state.time.to_ns){state.time[key==='from_ns'?'to_ns':'from_ns']=state.time[key]+(key==='from_ns'?element.step:-element.step);id(key==='from_ns'?'time-high':'time-low').value=state.time[key==='from_ns'?'to_ns':'from_ns'];}zoom=null;render();});}; configureTime(id('time-low'),'from_ns');configureTime(id('time-high'),'to_ns');
    const configurePct=(element,key)=>{element.value=state.percentile[key];element.addEventListener('input',()=>{state.percentile[key]=Number(element.value);if(state.percentile.low>=state.percentile.high){state.percentile[key==='low'?'high':'low']=state.percentile[key]+(key==='low'?1:-1);id(key==='low'?'pct-high':'pct-low').value=state.percentile[key==='low'?'high':'low'];}zoom=null;render();});}; configurePct(id('pct-low'),'low');configurePct(id('pct-high'),'high'); id('metric').value=state.metric; id('metric').addEventListener('change',e=>{state.metric=e.target.value;zoom=null;render();}); id('reset-zoom').addEventListener('click',()=>{zoom=null;render();}); const quality=profile.quality; id('quality').textContent=`Profile quality · ${quality.incomplete_invocations||quality.events_dropped||quality.samples_dropped?'warnings present':'complete capture data'}`;
  }
  bind(); render();
})();
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use slice_core::tail_divergence_profile;

    #[test]
    fn report_is_one_file_and_contains_the_tail_culprit() {
        let profile = tail_divergence_profile();
        let html = render_html(
            &profile,
            &Query {
                function_id: 1,
                threads: None,
                time: None,
                percentile: PercentileRange { low: 99, high: 100 },
                metric: Metric::Wall,
            },
        )
        .unwrap();
        assert!(html.contains("SliceFixture::slow_tail_b()"));
        assert!(!html.contains("https://"));
        assert!(!html.contains("fetch("));
        assert!(html.contains("id=\"pct-low\""));
    }
}
