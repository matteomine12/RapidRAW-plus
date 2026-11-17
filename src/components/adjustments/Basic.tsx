import { motion } from 'framer-motion';
import clsx from 'clsx';
import Slider from '../ui/Slider';
import Switch from '../ui/Switch';
import Text from '../ui/Text';
import { TextVariants } from '../../types/typography';
import { formatNits, useHdrDisplayInfo } from '../../hooks/useHdrDisplayInfo';
import { Adjustments, BasicAdjustment } from '../../utils/adjustments';
import { useEffect, useRef, useState, useMemo, memo } from 'react';
import { useTranslation } from 'react-i18next';
import { areSectionPropsEqual } from '../../utils/adjustments';

interface BasicAdjustmentsProps {
  adjustments: Adjustments;
  setAdjustments(adjustments: Partial<Adjustments>): any;
  isForMask?: boolean;
  onDragStateChange?: (isDragging: boolean) => void;
  appSettings?: any;
}

interface ToneMapperSwitchProps {
  selectedMapper: string;
  onMapperChange: (mapper: string) => void;
  exposureValue: number;
  onExposureChange: (value: number) => void;
  onDragStateChange?: (isDragging: boolean) => void;
}

const ToneMapperSwitch = ({
  selectedMapper,
  onMapperChange,
  exposureValue,
  onExposureChange,
  onDragStateChange,
}: ToneMapperSwitchProps) => {
  const { t } = useTranslation();
  const [bubbleStyle, setBubbleStyle] = useState({});
  const isInitialAnimation = useRef(true);
  const [isLabelHovered, setIsLabelHovered] = useState(false);

  const toneMapperOptions = useMemo(
    () => [
      {
        id: 'basic',
        label: t('adjustments.basic.mappers.basic'),
        title: t('adjustments.basic.mappers.basicDesc'),
      },
      {
        id: 'agx',
        label: t('adjustments.basic.mappers.agx'),
        title: t('adjustments.basic.mappers.agxDesc'),
      },
    ],
    [t],
  );

  const handleReset = () => {
    onMapperChange('basic');
    onExposureChange(0);
  };

  useEffect(() => {
    const selectedIndex = toneMapperOptions.findIndex((m) => m.id === selectedMapper);
    const safeIndex = selectedIndex >= 0 ? selectedIndex : 0;

    const widthPercent = 100 / toneMapperOptions.length;
    const targetX = `${safeIndex * 100}%`;
    const targetWidth = `${widthPercent}%`;

    if (isInitialAnimation.current) {
      let initialX;
      if (selectedMapper === 'agx') {
        initialX = `${toneMapperOptions.length * 100}%`;
      } else {
        initialX = '-25%';
      }

      setBubbleStyle({
        x: [initialX, targetX],
        width: targetWidth,
      });
      isInitialAnimation.current = false;
    } else {
      setBubbleStyle({
        x: targetX,
        width: targetWidth,
      });
    }
  }, [selectedMapper, toneMapperOptions]);

  return (
    <div className="group mb-3">
      <div className="flex justify-between items-center mb-2">
        <div
          className="grid cursor-pointer"
          onClick={handleReset}
          onDoubleClick={handleReset}
          onMouseEnter={() => setIsLabelHovered(true)}
          onMouseLeave={() => setIsLabelHovered(false)}
        >
          <span
            aria-hidden={isLabelHovered}
            className={`col-start-1 row-start-1 text-sm font-medium text-text-secondary select-none transition-opacity duration-200 ease-in-out ${
              isLabelHovered ? 'opacity-0' : 'opacity-100'
            }`}
          >
            {t('adjustments.basic.toneMapper')}
          </span>
          <span
            aria-hidden={!isLabelHovered}
            className={`col-start-1 row-start-1 text-sm font-medium text-text-primary select-none transition-opacity duration-200 ease-in-out pointer-events-none ${
              isLabelHovered ? 'opacity-100' : 'opacity-0'
            }`}
          >
            {t('adjustments.basic.reset')}
          </span>
        </div>
      </div>
      <div className="w-full p-2 pb-1 bg-card-active rounded-md">
        <div className="relative flex w-full">
          <motion.div
            className="absolute top-0 bottom-0 z-0 bg-accent"
            style={{ borderRadius: 6 }}
            animate={bubbleStyle}
            transition={{ type: 'spring', bounce: 0.2, duration: 0.6 }}
          />
          {toneMapperOptions.map((mapper) => (
            <button
              key={mapper.id}
              data-tooltip={mapper.title}
              onClick={() => onMapperChange(mapper.id)}
              className={clsx(
                'relative flex-1 flex items-center justify-center gap-2 px-3 p-1.5 text-sm font-medium rounded-md transition-colors',
                {
                  'text-text-primary hover:bg-surface': selectedMapper !== mapper.id,
                  'text-button-text': selectedMapper === mapper.id,
                },
              )}
              style={{ WebkitTapHighlightColor: 'transparent' }}
            >
              <span className="relative z-10 flex items-center">{mapper.label}</span>
            </button>
          ))}
        </div>
        <div className="mt-2.5 px-1">
          <Slider
            label={t('adjustments.basic.exposure')}
            max={5}
            min={-5}
            onChange={(e: any) => onExposureChange(parseFloat(e.target.value))}
            speedEditId={BasicAdjustment.Exposure}
            step={0.01}
            resetKey="exposure"
            value={exposureValue}
            trackClassName="bg-surface"
            onDragStateChange={onDragStateChange}
          />
        </div>
      </div>
    </div>
  );
};

function BasicAdjustments({
  adjustments,
  setAdjustments,
  isForMask = false,
  onDragStateChange,
  appSettings,
}: BasicAdjustmentsProps) {
  const { t } = useTranslation();

  const handleAdjustmentChange = (key: BasicAdjustment, value: any) => {
    const numericValue = parseFloat(value);
    setAdjustments((prev: Partial<Adjustments>) => ({ ...prev, [key]: numericValue }));
  };

  const handleToneMapperChange = (mapper: string) => {
    setAdjustments((prev: Partial<Adjustments>) => ({
      ...prev,
      toneMapper: mapper as 'basic' | 'agx',
    }));
  };

  const hideTonemapper = isForMask || appSettings?.tonemapperOverrideEnabled;

  const hdrDisplay = useHdrDisplayInfo();
  const sdrWhiteNits = hdrDisplay?.sdrWhiteNits ?? 203;
  // the most headroom the display can show, in stops above SDR white (null when the display is not in HDR)
  const displayEv = hdrDisplay?.active ? Math.min(Math.max(hdrDisplay.headroomEv, 0), 5) : null;

  const handleHdrToggle = (enabled: boolean) => {
    setAdjustments((prev: Partial<Adjustments>) => ({
      ...prev,
      hdrEnabled: enabled,
      // switching HDR on starts at what the display can show
      ...(enabled && displayEv !== null ? { hdrMaxEv: Math.round(displayEv * 10) / 10 } : {}),
    }));
  };

  const handleHdrHeadroomChange = (value: any) => {
    setAdjustments((prev: Partial<Adjustments>) => ({ ...prev, hdrMaxEv: parseFloat(value) }));
  };

  return (
    <div>
      {hideTonemapper ? (
        <Slider
          resetKey="exposure"
          label={t('adjustments.basic.exposure')}
          max={5}
          min={-5}
          onChange={(e: any) => handleAdjustmentChange(BasicAdjustment.Exposure, e.target.value)}
          speedEditId={BasicAdjustment.Exposure}
          step={0.01}
          value={adjustments.exposure}
          onDragStateChange={onDragStateChange}
        />
      ) : (
        <ToneMapperSwitch
          selectedMapper={adjustments.toneMapper || 'agx'}
          onMapperChange={handleToneMapperChange}
          exposureValue={adjustments.exposure}
          onExposureChange={(value) => handleAdjustmentChange(BasicAdjustment.Exposure, value)}
          onDragStateChange={onDragStateChange}
        />
      )}
      <Slider
        resetKey="contrast"
        label={t('adjustments.basic.contrast')}
        max={100}
        min={-100}
        onChange={(e: any) => handleAdjustmentChange(BasicAdjustment.Contrast, e.target.value)}
        speedEditId={BasicAdjustment.Contrast}
        step={1}
        value={adjustments.contrast}
        onDragStateChange={onDragStateChange}
      />
      <Slider
        resetKey="highlights"
        label={t('adjustments.basic.highlights')}
        max={100}
        min={-100}
        onChange={(e: any) => handleAdjustmentChange(BasicAdjustment.Highlights, e.target.value)}
        speedEditId={BasicAdjustment.Highlights}
        step={1}
        value={adjustments.highlights}
        onDragStateChange={onDragStateChange}
      />
      <Slider
        resetKey="shadows"
        label={t('adjustments.basic.shadows')}
        max={100}
        min={-100}
        onChange={(e: any) => handleAdjustmentChange(BasicAdjustment.Shadows, e.target.value)}
        speedEditId={BasicAdjustment.Shadows}
        step={1}
        value={adjustments.shadows}
        onDragStateChange={onDragStateChange}
      />
      <Slider
        resetKey="whites"
        label={t('adjustments.basic.whites')}
        max={100}
        min={-100}
        onChange={(e: any) => handleAdjustmentChange(BasicAdjustment.Whites, e.target.value)}
        speedEditId={BasicAdjustment.Whites}
        step={1}
        value={adjustments.whites}
        onDragStateChange={onDragStateChange}
      />
      <Slider
        resetKey="blacks"
        label={t('adjustments.basic.blacks')}
        max={100}
        min={-100}
        onChange={(e: any) => handleAdjustmentChange(BasicAdjustment.Blacks, e.target.value)}
        speedEditId={BasicAdjustment.Blacks}
        step={1}
        value={adjustments.blacks}
        onDragStateChange={onDragStateChange}
      />
      <Slider
        resetKey="brightness"
        label={t('adjustments.basic.brightness')}
        max={5}
        min={-5}
        onChange={(e: any) => handleAdjustmentChange(BasicAdjustment.Brightness, e.target.value)}
        step={0.01}
        value={adjustments.brightness}
        onDragStateChange={onDragStateChange}
      />
      {!isForMask && (
        <>
          <div className="mt-3 mb-1">
            <Switch
              checked={adjustments.hdrEnabled ?? false}
              id="hdr-edit-toggle"
              label={t('adjustments.basic.hdr')}
              onChange={handleHdrToggle}
            />
          </div>
          {adjustments.hdrEnabled && (
            <>
              <Slider
                resetKey="hdrMaxEv"
                label={
                  <>
                    {t('adjustments.basic.hdrHeadroom')}{' '}
                    <span className="opacity-60">
                      {formatNits(sdrWhiteNits * 2 ** (adjustments.hdrMaxEv ?? 3))} {t('adjustments.basic.hdrNits')}
                    </span>
                  </>
                }
                max={5}
                min={0}
                defaultValue={displayEv !== null ? Math.round(displayEv * 10) / 10 : 3}
                markers={displayEv !== null ? [{ color: '#FFB84D', value: displayEv }] : undefined}
                onChange={(e: any) => handleHdrHeadroomChange(e.target.value)}
                step={0.1}
                value={adjustments.hdrMaxEv ?? 3}
                onDragStateChange={onDragStateChange}
              />
              <Text variant={TextVariants.small} className="mb-2">
                {hdrDisplay?.active
                  ? t('adjustments.basic.hdrDisplayInfo', {
                      peak: formatNits(hdrDisplay.peakNits),
                      ev: (displayEv ?? 0).toFixed(1),
                    })
                  : t('adjustments.basic.hdrNoDisplay')}
              </Text>
            </>
          )}
        </>
      )}
    </div>
  );
}

export default memo(BasicAdjustments, areSectionPropsEqual('basic'));
