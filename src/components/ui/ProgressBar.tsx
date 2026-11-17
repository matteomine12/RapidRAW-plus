import clsx from 'clsx';
import { motion } from 'framer-motion';
import { SPRING } from '../../utils/motion';

interface ProgressBarProps {
  /** Progress from 0 to 100. */
  value: number;
  className?: string;
  fillClassName?: string;
  /** Track thickness. M3 Expressive uses a thick, fully rounded track with a stop indicator. */
  size?: 'sm' | 'md';
  label?: string;
}

/** Linear progress: thick rounded track, spring-driven fill, and a stop dot marking the end. */
export default function ProgressBar({ value, className, fillClassName, size = 'md', label }: ProgressBarProps) {
  const pct = Math.max(0, Math.min(100, Number.isFinite(value) ? value : 0));
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(pct)}
      aria-label={label}
      className={clsx(
        'relative w-full overflow-hidden rounded-full bg-card-active',
        size === 'md' ? 'h-2.5' : 'h-1.5',
        className,
      )}
    >
      <motion.div
        className={clsx('h-full rounded-full bg-accent', fillClassName)}
        initial={false}
        animate={{ width: `${pct}%` }}
        transition={SPRING.gentle}
      />
      <span
        aria-hidden
        className={clsx(
          'absolute top-1/2 right-1 -translate-y-1/2 rounded-full bg-text-secondary',
          size === 'md' ? 'h-1 w-1' : 'h-0.5 w-0.5',
          pct > 96 && 'opacity-0',
        )}
      />
    </div>
  );
}
