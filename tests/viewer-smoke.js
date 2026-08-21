#!/usr/bin/env node

// A dependency-free DOM smoke test for the offline report. This is not a
// substitute for visual browser testing; it proves that the generated script
// executes against the DOM contract it emits and creates the observable UI.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const reportPath = process.argv[2];
assert(reportPath, 'usage: viewer-smoke.js REPORT.html');
const html = fs.readFileSync(reportPath, 'utf8');
assert(!/<script[^>]+src=/i.test(html), 'offline report must not load scripts');
assert(!html.includes('fetch('), 'offline report must not fetch data');

class Element {
  constructor(tagName = 'div') {
    this.tagName = tagName;
    this.children = [];
    this.attributes = new Map();
    this.listeners = new Map();
    this.style = {};
    this.hidden = false;
    this.value = '';
    this.checked = false;
    this.scrollTop = 0;
    this.scrollLeft = 0;
    this.clientWidth = 800;
    this.clientHeight = 220;
    this._textContent = '';
    this._innerHTML = '';
  }

  set textContent(value) { this._textContent = String(value); }
  get textContent() { return this._textContent; }
  set innerHTML(value) { this._innerHTML = String(value); this.children = []; }
  get innerHTML() { return this._innerHTML; }
  setAttribute(name, value) { this.attributes.set(name, String(value)); }
  getAttribute(name) { return this.attributes.get(name) ?? null; }
  appendChild(child) { this.children.push(child); return child; }
  append(...children) { children.forEach(child => this.appendChild(child)); }
  addEventListener(name, handler) {
    if (!this.listeners.has(name)) this.listeners.set(name, []);
    this.listeners.get(name).push(handler);
  }
  dispatchEvent(event) {
    event.target ||= this;
    this[`on${event.type}`]?.(event);
    for (const handler of this.listeners.get(event.type) ?? []) handler(event);
  }
  click() { this.dispatchEvent({ type: 'click', target: this }); }
  setPointerCapture() {}
  releasePointerCapture() {}
  getBoundingClientRect() { return { left: 0, top: 0, width: this.clientWidth, height: this.clientHeight }; }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  querySelectorAll(selector) {
    const matches = (element) => {
      if (selector.startsWith('.')) return element.getAttribute('class')?.split(/\s+/).includes(selector.slice(1));
      if (selector === 'input') return element.tagName === 'input';
      if (selector === 'title') return element.tagName === 'title';
      const dataHandle = selector.match(/^\[data-handle="([^"]+)"\]$/);
      return dataHandle && element.getAttribute('data-handle') === dataHandle[1];
    };
    const found = [];
    const visit = (element) => {
      for (const child of element.children) {
        if (matches(child)) found.push(child);
        visit(child);
      }
    };
    visit(this);
    return found;
  }
  get classList() {
    const classes = () => (this.getAttribute('class') ?? '').split(/\s+/).filter(Boolean);
    return { contains: name => classes().includes(name) };
  }
}

const elements = new Map();
for (const match of html.matchAll(/\bid="([^"]+)"/g)) elements.set(match[1], new Element('div'));
for (const id of ['viewer-error', 'empty', 'flame-tooltip']) elements.get(id).hidden = true;
const scriptContent = (id) => {
  const match = html.match(new RegExp(`<script[^>]*\\bid="${id}"[^>]*>([\\s\\S]*?)</script>`));
  assert(match, `missing ${id} script`);
  return match[1];
};
elements.get('slice-profile').textContent = scriptContent('slice-profile');
elements.get('slice-initial-query').textContent = scriptContent('slice-initial-query');
const scripts = [...html.matchAll(/<script(?:[^>]*)>([\s\S]*?)<\/script>/g)];
const javascript = scripts.at(-1)[1];
assert(javascript.trim().startsWith('(() =>'), 'viewer script was not embedded');

const document = {
  getElementById(id) { const element = elements.get(id); assert(element, `missing element #${id}`); return element; },
  createElement(tagName) { return new Element(tagName); },
  createElementNS(_namespace, tagName) { return new Element(tagName); },
  createTextNode(value) { const node = new Element('#text'); node.textContent = value; return node; },
};
const window = { innerWidth: 1280, innerHeight: 720, addEventListener() {} };
const context = {
  document,
  window,
  MutationObserver: class { observe() {} },
  console,
  JSON,
  Map,
  Set,
  Math,
  Number,
  String,
  Array,
  Error,
};
vm.runInNewContext(javascript, context, { filename: reportPath });

assert(elements.get('population-name').textContent.includes('BimodalFixture::handle_request'));
assert(!html.includes('Firefox Profiler-inspired execution explorer'));
assert(!html.includes('PERCENTILE-CONDITIONED PROFILE'));
assert(!html.includes('Profile quality · complete capture data'));
assert(elements.get('timeline').children.length > 10, 'timeline should contain lanes and activity');
assert(elements.get('histogram').children.length > 10, 'histogram should contain bins and handles');
assert(elements.get('histogram').querySelectorAll('.hist-label').length >= 5, 'histogram should contain intermediate time labels');
assert(elements.get('flame').children.length > 0, 'flame graph should contain frames');
assert(elements.get('flame').querySelectorAll('.frame').every(frame => frame.getAttribute('tabindex') === '0'));
assert(elements.get('timeline').querySelectorAll('.timeline-invocation').every(bar => bar.getAttribute('aria-label')));
assert(elements.get('threads').children.length === 10, 'all fixture threads should be observable');
assert(elements.get('empty').hidden, 'fixture query should not be empty');
assert(elements.get('viewer-error').hidden, 'viewer should not report a runtime error');

const timeline = elements.get('timeline');
const initialCaptureEnd = Number(elements.get('time-high').value);
timeline.dispatchEvent({ type: 'pointerdown', pointerId: 7, clientX: 160, preventDefault() {} });
timeline.dispatchEvent({ type: 'pointermove', pointerId: 7, clientX: 640, preventDefault() {} });
timeline.dispatchEvent({ type: 'pointerup', pointerId: 7, clientX: 640, preventDefault() {} });
const selectedFrom = Number(elements.get('time-low').value);
const selectedTo = Number(elements.get('time-high').value);
assert(selectedFrom > 0 && selectedTo < initialCaptureEnd, 'timeline drag should create an interior time range');
assert(selectedFrom < selectedTo, 'timeline drag should preserve ordered time bounds');

const histogram = elements.get('histogram');
const bucketSize = elements.get('histogram-bucket-size');
assert(bucketSize.value === 'auto', 'histogram should default to automatic bucket sizing');
const automaticBucketCount = histogram.querySelectorAll('.hist-bar').length;
bucketSize.value = '1000000';
bucketSize.dispatchEvent({ type: 'change', target: bucketSize });
const fixedBucketCount = histogram.querySelectorAll('.hist-bar').length;
assert(fixedBucketCount !== automaticBucketCount, 'histogram bucket size should change the bin count');
const oneMsPopulatedBins = [...histogram.querySelectorAll('.hist-bar')].filter(bar => !bar.classList.contains('hist-empty')).length;
bucketSize.value = '250000';
bucketSize.dispatchEvent({ type: 'change', target: bucketSize });
const quarterMsBucketCount = histogram.querySelectorAll('.hist-bar').length;
const quarterMsPopulatedBins = [...histogram.querySelectorAll('.hist-bar')].filter(bar => !bar.classList.contains('hist-empty')).length;
assert(quarterMsBucketCount > fixedBucketCount, 'quarter-millisecond buckets should create more intervals');
assert(quarterMsPopulatedBins !== oneMsPopulatedBins, 'quarter-millisecond buckets should redistribute observations');
const firstHistogramBar = histogram.querySelector('.hist-bar');
assert(firstHistogramBar.getAttribute('data-frequency-range'), 'histogram buckets should expose a latency range');
firstHistogramBar.dispatchEvent({ type: 'mouseenter', clientX: 250, clientY: 40 });
assert(elements.get('flame-tooltip').innerHTML.includes('Histogram bucket'), 'histogram hover should identify the bucket');
assert(elements.get('flame-tooltip').innerHTML.includes('Samples (invocations):'), 'histogram hover should report the sample count');
firstHistogramBar.dispatchEvent({ type: 'mouseleave' });
assert(elements.get('flame-tooltip').hidden, 'histogram tooltip should hide after hover');
const initialHistogramWindow = histogram.querySelector('.hist-window');
const initialHistogramX = Number(initialHistogramWindow.getAttribute('x'));
histogram.dispatchEvent({ type: 'pointerdown', pointerId: 8, clientX: 150, clientY: 70, preventDefault() {} });
histogram.dispatchEvent({ type: 'pointermove', pointerId: 8, clientX: 600, clientY: 70, preventDefault() {} });
histogram.dispatchEvent({ type: 'pointerup', pointerId: 8, clientX: 600, clientY: 70, preventDefault() {} });
const selectedHistogramX = Number(histogram.querySelector('.hist-window').getAttribute('x'));
assert(selectedHistogramX !== initialHistogramX, 'histogram drag should create a new latency range');

const movedHistogramX = Number(histogram.querySelector('.hist-window').getAttribute('x'));
histogram.dispatchEvent({ type: 'pointerdown', pointerId: 9, clientX: 300, clientY: 8, preventDefault() {} });
histogram.dispatchEvent({ type: 'pointermove', pointerId: 9, clientX: 360, clientY: 8, preventDefault() {} });
histogram.dispatchEvent({ type: 'pointerup', pointerId: 9, clientX: 360, clientY: 8, preventDefault() {} });
const shiftedHistogramX = Number(histogram.querySelector('.hist-window').getAttribute('x'));
assert(shiftedHistogramX !== movedHistogramX, 'histogram rail drag should move the latency window');

console.log('viewer-smoke: generated report executed and rendered observable controls');
