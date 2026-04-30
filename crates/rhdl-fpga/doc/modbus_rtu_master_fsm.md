

<p>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 310 1010" font-family="sans-serif" font-size="13">
<defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="#444"/></marker></defs>
<title>FSM diagram for ModbusRtuMaster</title>
<path d="M 143 30 C 130 -5, 180 -5, 167 30" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="155" y1="70" x2="155" y2="160" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 143 160 C 130 125, 180 125, 167 160" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="155" y1="200" x2="155" y2="290" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 143 290 C 130 255, 180 255, 167 290" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="155" y1="330" x2="155" y2="420" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 143 420 C 130 385, 180 385, 167 420" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="155" y1="460" x2="155" y2="550" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 143 550 C 130 515, 180 515, 167 550" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="155" y1="590" x2="155" y2="680" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 143 680 C 130 645, 180 645, 167 680" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="155" y1="720" x2="155" y2="810" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="155" y1="850" x2="85" y2="940" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="155" y1="850" x2="225" y2="940" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 73 940 C 60 905, 110 905, 97 940" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="85" y1="980" x2="225" y2="940" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="225" y1="980" x2="155" y2="30" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<rect x="100" y="30" width="110" height="40" rx="6" ry="6" fill="#e0f2ff" stroke="#2563eb" stroke-width="1"/>
<text x="155" y="55" text-anchor="middle">idle</text>
<rect x="100" y="160" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="155" y="185" text-anchor="middle">build req</text>
<rect x="100" y="290" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="155" y="315" text-anchor="middle">req CRC</text>
<rect x="100" y="420" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="155" y="445" text-anchor="middle">send</text>
<rect x="100" y="550" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="155" y="575" text-anchor="middle">rx wait</text>
<rect x="100" y="680" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="155" y="705" text-anchor="middle">rx</text>
<rect x="100" y="810" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="155" y="835" text-anchor="middle">validate</text>
<rect x="30" y="940" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="85" y="965" text-anchor="middle">decode</text>
<rect x="170" y="940" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="225" y="965" text-anchor="middle">done</text>
</svg>

</p>