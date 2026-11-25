// Seven-segment frequency readout drawn as SVG, laid out like a radio display:
// MHz.kHz.(100 Hz)(10 Hz), e.g. "14.074.00" or "145.500.00". Unlit segments
// stay faintly visible, like a real LCD.

(function () {
  const NS = "http://www.w3.org/2000/svg";
  const W = 50, H = 100, T = 9, G = 1.6;
  const CELL = 64, DOT = 22;

  function hseg(x1, x2, y) {
    const h = T / 2;
    return [[x1, y], [x1 + h, y - h], [x2 - h, y - h], [x2, y], [x2 - h, y + h], [x1 + h, y + h]];
  }
  function vseg(x, y1, y2) {
    const h = T / 2;
    return [[x, y1], [x + h, y1 + h], [x + h, y2 - h], [x, y2], [x - h, y2 - h], [x - h, y1 + h]];
  }
  const x0 = T / 2, x1 = W - T / 2, ym = H / 2;
  const SEG = {
    a: hseg(x0 + G, x1 - G, T / 2),
    g: hseg(x0 + G, x1 - G, ym),
    d: hseg(x0 + G, x1 - G, H - T / 2),
    f: vseg(x0, T / 2 + G, ym - G),
    b: vseg(x1, T / 2 + G, ym - G),
    e: vseg(x0, ym + G, H - T / 2 - G),
    c: vseg(x1, ym + G, H - T / 2 - G),
  };
  const DIGITS = {
    "0": "abcdef", "1": "bc", "2": "abged", "3": "abgcd", "4": "fgbc",
    "5": "afgcd", "6": "afgedc", "7": "abc", "8": "abcdefg", "9": "abcdfg",
    "-": "g", " ": "",
  };

  function build(svg) {
    svg.innerHTML = "";
    const slots = [];
    const layout = ["d", "d", "d", ".", "d", "d", "d", ".", "d", "d"];
    let x = 0;
    const root = document.createElementNS(NS, "g");
    for (const kind of layout) {
      if (kind === ".") {
        const r = document.createElementNS(NS, "rect");
        r.setAttribute("x", x + 2); r.setAttribute("y", H - 10);
        r.setAttribute("width", 10); r.setAttribute("height", 10);
        r.setAttribute("class", "on");
        root.appendChild(r);
        x += DOT;
        continue;
      }
      const g = document.createElementNS(NS, "g");
      g.setAttribute("transform", `translate(${x + 6},4) skewX(-7)`);
      const parts = {};
      for (const [name, pts] of Object.entries(SEG)) {
        const p = document.createElementNS(NS, "polygon");
        p.setAttribute("points", pts.map(([a, b]) => `${a},${b}`).join(" "));
        p.setAttribute("class", "off");
        g.appendChild(p);
        parts[name] = p;
      }
      root.appendChild(g);
      slots.push(parts);
      x += CELL;
    }
    svg.appendChild(root);
    svg.setAttribute("viewBox", `0 0 ${x} ${H + 8}`);
    svg._slots = slots;
  }

  function setDigit(parts, ch) {
    const lit = DIGITS[ch] ?? "";
    for (const [name, el] of Object.entries(parts)) {
      el.setAttribute("class", lit.includes(name) ? "on" : "off");
    }
  }

  // hz: number or null (null shows dashes)
  window.renderFrequency = function (svg, hz) {
    if (!svg._slots) build(svg);
    let chars;
    if (hz == null || !isFinite(hz)) {
      chars = "--- --- --".replace(/ /g, "").split("");
    } else {
      const tens = Math.round(hz / 10);
      const mhz = Math.floor(tens / 100000);
      const khz = Math.floor(tens / 100) % 1000;
      const sub = tens % 100;
      const m = String(mhz % 1000).padStart(3, " ");
      chars = (m + String(khz).padStart(3, "0") + String(sub).padStart(2, "0")).split("");
    }
    svg._slots.forEach((parts, i) => setDigit(parts, chars[i]));
  };
})();
