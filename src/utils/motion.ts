import type { Transition } from 'framer-motion';

/** Shared spring presets. Every animated surface should pick one of these instead of ad-hoc tweens. */
export const SPRING = {
  /** Panels, modals, layout changes. */
  standard: { type: 'spring', stiffness: 380, damping: 32, mass: 0.9 } as Transition,
  /** Playful overshoot for emphasis: selection, toggles, popovers. */
  bouncy: { type: 'spring', stiffness: 460, damping: 24, mass: 0.8 } as Transition,
  /** Press and hover feedback. */
  snappy: { type: 'spring', stiffness: 700, damping: 38, mass: 0.6 } as Transition,
  /** Slow, soft movement for big surfaces. */
  gentle: { type: 'spring', stiffness: 180, damping: 26, mass: 1 } as Transition,
} as const;

export const PRESS = { whileTap: { scale: 0.95 }, whileHover: { scale: 1.03 }, transition: SPRING.snappy };
