import React, { useId } from 'react';
import clsx from 'clsx';
import { motion } from 'framer-motion';
import Text from './Text';
import { TextVariants } from '../../types/typography';
import { SPRING } from '../../utils/motion';

interface SwitchProps {
  checked: boolean;
  className?: string;
  disabled?: boolean;
  id?: string;
  label: string;
  onChange(val: boolean): any;
  tooltip?: string;
  trackClassName?: string;
}

/**
 * A beautiful, reusable, and accessible toggle switch component.
 *
 * @param {string} label - The text label for the switch.
 * @param {boolean} checked - The current state of the switch.
 * @param {function(boolean): void} onChange - Callback function that receives the new boolean state.
 * @param {boolean} [disabled=false] - Whether the switch is interactive.
 * @param {string} [className=''] - Additional classes for the container.
 * @param {string} [trackClassName] - Custom classes for the switch's background track.
 */
const Switch = ({
  checked,
  className = '',
  disabled = false,
  id,
  label,
  onChange,
  tooltip,
  trackClassName,
}: SwitchProps) => {
  const generatedId = useId();
  const uniqueId = id ?? generatedId;

  const spring = SPRING.standard;

  return (
    <label
      className={clsx(
        'flex items-center justify-between',
        disabled ? 'cursor-not-allowed opacity-50' : 'cursor-pointer',
        className,
      )}
      htmlFor={uniqueId}
      data-tooltip={tooltip}
    >
      <Text variant={TextVariants.label} className="select-none">
        {label}
      </Text>
      <div className="relative w-12 h-7">
        <input
          checked={checked}
          className="sr-only"
          disabled={disabled}
          id={uniqueId}
          onChange={(e: any) => !disabled && onChange(e.target.checked)}
          type="checkbox"
        />
        <div
          className={clsx(
            'w-full h-full rounded-full border-2 transition-colors',
            checked ? 'bg-accent border-accent' : 'bg-card-active border-text-secondary/60',
            trackClassName,
          )}
        ></div>
        <motion.div
          className={clsx('absolute top-1.5 left-1.5 h-4 w-4 rounded-full', {
            'bg-button-text': checked,
            'bg-text-secondary': !checked,
          })}
          transition={spring}
          initial={false}
          whileTap={{ scale: 1.35 }}
          animate={{ x: checked ? 20 : 0 }}
        />
      </div>
    </label>
  );
};

export default Switch;
