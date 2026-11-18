import { useCallback, useMemo, useRef, useState } from 'react';
import { motion } from 'framer-motion';
import clsx from 'clsx';
import { SPRING } from '../../utils/motion';
import { lookGradient, SmartLook, SMART_LUT_MAX, SMART_LUT_NEUTRAL } from '../../utils/smartLut';

const DOTS = 11;
const STEP = 5;

interface SmartLutPadProps {
  look: Pick<SmartLook, 'palette' | 'monochrome'>;
  /** 0..100, horizontal (right = more). */
  color: number;
  /** 0..100, vertical (up = more). */
  luminance: number;
  disabled?: boolean;
  colorLabel: string;
  luminanceLabel: string;
  onChange(color: number, luminance: number): void;
  onDragStateChange?(dragging: boolean): void;
}

const clamp = (v: number) => Math.min(SMART_LUT_MAX, Math.max(0, v));

/**
 * The look's colour × luminance pad: a square painted with the look's colours and a grid of dots, the row and
 * column through the handle lit up. Drag (or use the arrow keys) to move; double-click returns to the whole look
 * (top right). The ring at the bottom left marks no effect.
 */
export default function SmartLutPad({
  look,
  color,
  luminance,
  disabled,
  colorLabel,
  luminanceLabel,
  onChange,
  onDragStateChange,
}: SmartLutPadProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  const x = color / SMART_LUT_MAX;
  const y = 1 - luminance / SMART_LUT_MAX;
  const column = Math.round(x * (DOTS - 1));
  const row = Math.round(y * (DOTS - 1));
  const background = useMemo(() => lookGradient(look), [look]);

  const moveTo = useCallback(
    (clientX: number, clientY: number) => {
      const rect = ref.current?.getBoundingClientRect();
      if (!rect) return;
      // the handle runs between the centres of the outer dots
      const along = (offset: number, size: number) =>
        Math.min(1, Math.max(0, ((offset / size) * DOTS - 0.5) / (DOTS - 1)));
      const fx = along(clientX - rect.left, rect.width);
      const fy = along(clientY - rect.top, rect.height);
      onChange(Math.round(fx * SMART_LUT_MAX), Math.round((1 - fy) * SMART_LUT_MAX));
    },
    [onChange],
  );

  const handlePointerDown = (e: React.PointerEvent) => {
    if (disabled || e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDragging(true);
    onDragStateChange?.(true);
    moveTo(e.clientX, e.clientY);
  };

  const handlePointerMove = (e: React.PointerEvent) => {
    if (dragging) moveTo(e.clientX, e.clientY);
  };

  const endDrag = (e: React.PointerEvent) => {
    if (!dragging) return;
    e.currentTarget.releasePointerCapture(e.pointerId);
    setDragging(false);
    onDragStateChange?.(false);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (disabled) return;
    const step = e.shiftKey ? STEP * 4 : STEP;
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-step, 0],
      ArrowRight: [step, 0],
      ArrowUp: [0, step],
      ArrowDown: [0, -step],
    };
    const move = moves[e.key];
    if (!move) return;
    e.preventDefault();
    onChange(clamp(color + move[0]), clamp(luminance + move[1]));
  };

  const dots = [];
  for (let r = 0; r < DOTS; r++) {
    for (let c = 0; c < DOTS; c++) {
      const onLine = r === row || c === column;
      if (r === row && c === column) continue;
      const isOrigin = r === DOTS - 1 && c === 0;
      const fade = 0.45;
      dots.push(
        <circle
          key={`${r}-${c}`}
          cx={(c + 0.5) * (100 / DOTS)}
          cy={(r + 0.5) * (100 / DOTS)}
          r={isOrigin && !onLine ? 1.45 : onLine ? 1.15 : 0.85}
          fill={isOrigin && !onLine ? 'none' : 'white'}
          stroke={isOrigin && !onLine ? 'white' : 'none'}
          strokeWidth={0.45}
          opacity={onLine ? 0.95 : isOrigin ? 0.7 : fade}
        />,
      );
    }
  }

  // amounts as a share of the look: 0 = no effect, 100 = the look as extracted
  const signed = (v: number) => `${Math.round(v)}%`;

  return (
    <div className="flex flex-col items-center gap-2 select-none">
      <div
        ref={ref}
        role="slider"
        tabIndex={disabled ? -1 : 0}
        aria-label={`${colorLabel} / ${luminanceLabel}`}
        aria-valuetext={`${colorLabel} ${signed(color)}, ${luminanceLabel} ${signed(luminance)}`}
        className={clsx(
          'relative w-full max-w-[260px] aspect-square rounded-[28px] overflow-hidden touch-none outline-none',
          'focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 focus-visible:ring-offset-bg-secondary',
          disabled ? 'opacity-60 cursor-default' : dragging ? 'cursor-grabbing' : 'cursor-pointer',
        )}
        style={{ background }}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onDoubleClick={() => !disabled && onChange(SMART_LUT_NEUTRAL, SMART_LUT_NEUTRAL)}
        onKeyDown={handleKeyDown}
      >
        <svg
          viewBox="0 0 100 100"
          className="absolute inset-0 w-full h-full pointer-events-none"
          style={{ mixBlendMode: 'difference' }}
        >
          {dots}
        </svg>
        <motion.div
          className="absolute w-[13%] aspect-square rounded-full bg-white pointer-events-none"
          style={{ boxShadow: '0 2px 10px rgba(0,0,0,0.35)', translateX: '-50%', translateY: '-50%' }}
          initial={false}
          animate={{
            left: `${((x * (DOTS - 1) + 0.5) / DOTS) * 100}%`,
            top: `${((y * (DOTS - 1) + 0.5) / DOTS) * 100}%`,
            scale: dragging ? 1.12 : 1,
          }}
          transition={dragging ? { duration: 0 } : SPRING.bouncy}
        />
      </div>
      <div className="w-full max-w-[260px] flex justify-between text-xs text-text-secondary tabular-nums">
        <span>
          {colorLabel} <span className="text-text-primary">{signed(color)}</span>
        </span>
        <span>
          {luminanceLabel} <span className="text-text-primary">{signed(luminance)}</span>
        </span>
      </div>
    </div>
  );
}
