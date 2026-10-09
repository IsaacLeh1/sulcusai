// SPDX-License-Identifier: AGPL-3.0-only
// Paint over part of a picture to say "change this". The strokes become the
// mask the core fills in (any painted pixel = change it).
import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { toBase64 } from "../api";

export interface MaskHandle {
  /** The painted area as a PNG (base64), or null if nothing is painted. */
  mask(): Promise<string | null>;
  clear(): void;
}

export const MaskPainter = forwardRef<MaskHandle, { src: string; width: number; height: number; brush: number; onPainted: (any: boolean) => void }>(
  function MaskPainter({ src, width, height, brush, onPainted }, ref) {
    const canvas = useRef<HTMLCanvasElement>(null);
    const drawing = useRef(false);
    const last = useRef<[number, number] | null>(null);
    const [painted, setPainted] = useState(false);

    useEffect(() => onPainted(painted), [painted, onPainted]);

    useImperativeHandle(ref, () => ({
      async mask() {
        const c = canvas.current;
        if (!c || !painted) return null;
        const blob: Blob | null = await new Promise((res) => c.toBlob(res, "image/png"));
        if (!blob) return null;
        return toBase64(new Uint8Array(await blob.arrayBuffer()));
      },
      clear() {
        const c = canvas.current;
        c?.getContext("2d")?.clearRect(0, 0, c.width, c.height);
        setPainted(false);
      },
    }));

    // Brush size is in screen pixels; the canvas is at the picture's size.
    const at = (e: React.PointerEvent<HTMLCanvasElement>): [number, number, number] => {
      const r = e.currentTarget.getBoundingClientRect();
      const scale = width / r.width;
      return [(e.clientX - r.left) * scale, (e.clientY - r.top) * scale, brush * scale];
    };

    const stroke = (e: React.PointerEvent<HTMLCanvasElement>) => {
      const ctx = canvas.current?.getContext("2d");
      if (!ctx) return;
      const [x, y, size] = at(e);
      ctx.strokeStyle = ctx.fillStyle = "rgb(255, 64, 64)";
      ctx.lineWidth = size;
      ctx.lineCap = ctx.lineJoin = "round";
      ctx.beginPath();
      const [px, py] = last.current ?? [x, y];
      ctx.moveTo(px, py);
      ctx.lineTo(x, y);
      ctx.stroke();
      last.current = [x, y];
      if (!painted) setPainted(true);
    };

    return (
      <div className="mask-painter">
        <img src={src} alt="" draggable={false} />
        <canvas
          ref={canvas}
          width={width}
          height={height}
          onPointerDown={(e) => {
            drawing.current = true;
            last.current = null;
            e.currentTarget.setPointerCapture(e.pointerId);
            stroke(e);
          }}
          onPointerMove={(e) => drawing.current && stroke(e)}
          onPointerUp={() => {
            drawing.current = false;
            last.current = null;
          }}
          aria-label="Paint over the area to change"
        />
      </div>
    );
  },
);
