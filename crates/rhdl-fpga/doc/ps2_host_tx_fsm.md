

<p>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 170 880" font-family="sans-serif" font-size="13">
<defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="#444"/></marker></defs>
<title>FSM diagram for Ps2HostTx</title>
<path d="M 73 30 C 60 -5, 110 -5, 97 30" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="85" y1="70" x2="85" y2="160" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 73 160 C 60 125, 110 125, 97 160" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="85" y1="200" x2="85" y2="290" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 73 290 C 60 255, 110 255, 97 290" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="85" y1="330" x2="85" y2="420" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 73 420 C 60 385, 110 385, 97 420" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="85" y1="460" x2="85" y2="550" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 73 550 C 60 515, 110 515, 97 550" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="85" y1="590" x2="85" y2="680" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 73 680 C 60 645, 110 645, 97 680" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="85" y1="720" x2="85" y2="810" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<line x1="85" y1="850" x2="85" y2="30" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<path d="M 73 810 C 60 775, 110 775, 97 810" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>
<rect x="30" y="30" width="110" height="40" rx="6" ry="6" fill="#e0f2ff" stroke="#2563eb" stroke-width="1"/>
<text x="85" y="55" text-anchor="middle">idle</text>
<rect x="30" y="160" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="85" y="185" text-anchor="middle">inhibit</text>
<rect x="30" y="290" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="85" y="315" text-anchor="middle">request start</text>
<rect x="30" y="420" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="85" y="445" text-anchor="middle">clock 8 bits</text>
<rect x="30" y="550" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="85" y="575" text-anchor="middle">clock parity</text>
<rect x="30" y="680" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="85" y="705" text-anchor="middle">clock stop</text>
<rect x="30" y="810" width="110" height="40" rx="6" ry="6" fill="#ffffff" stroke="#444" stroke-width="1"/>
<text x="85" y="835" text-anchor="middle">await ack</text>
</svg>

</p>