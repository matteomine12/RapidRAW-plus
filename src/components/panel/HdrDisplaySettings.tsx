import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import Dropdown from '../ui/Dropdown';
import Slider from '../ui/Slider';
import Switch from '../ui/Switch';
import Text from '../ui/Text';
import { TextVariants } from '../../types/typography';

interface DetectedHdr {
  hdrOn: boolean;
  peakNits?: number | null;
  sdrWhiteNits?: number | null;
}

export interface HdrDisplayInfo {
  supported: boolean;
  detected?: DetectedHdr | null;
  active: boolean;
  peakNits: number;
  sdrWhiteNits: number;
  headroomEv: number;
}

interface HdrDisplaySettingsProps {
  appSettings: any;
  onSettingsChange(settings: any): Promise<void>;
}

const PEAK_RANGE = { min: 200, max: 4000, step: 50 };
const SDR_WHITE_RANGE = { min: 80, max: 600, step: 5 };

const HdrDisplaySettings = ({ appSettings, onSettingsChange }: HdrDisplaySettingsProps) => {
  const { t } = useTranslation();
  const [info, setInfo] = useState<HdrDisplayInfo | null>(null);

  const mode: string = appSettings?.hdrDisplayMode ?? 'auto';
  const peakOverride: number | null = appSettings?.hdrPeakNits ?? null;
  const whiteOverride: number | null = appSettings?.hdrSdrWhiteNits ?? null;

  useEffect(() => {
    let disposed = false;
    invoke<HdrDisplayInfo>('get_hdr_display_info')
      .then((next) => !disposed && setInfo(next))
      .catch(console.error);
    const unlisten = listen<HdrDisplayInfo>('hdr-display-changed', (event) => setInfo(event.payload));
    return () => {
      disposed = true;
      unlisten.then((fn) => fn());
    };
  }, []);

  const update = useCallback(
    async (patch: Record<string, unknown>) => {
      await onSettingsChange({ ...appSettings, ...patch });
      try {
        setInfo(await invoke<HdrDisplayInfo>('apply_hdr_display_settings'));
      } catch (error) {
        console.error(error);
      }
    },
    [appSettings, onSettingsChange],
  );

  const peak = peakOverride ?? Math.round(info?.peakNits ?? 1000);
  const white = whiteOverride ?? Math.round(info?.sdrWhiteNits ?? 203);

  const detectedLine = info?.detected?.hdrOn
    ? t('settings.hdrDisplay.detected', {
        peak: Math.round(info.detected.peakNits ?? 0) || '?',
        white: Math.round(info.detected.sdrWhiteNits ?? 0) || '?',
      })
    : t('settings.hdrDisplay.notDetected');

  const statusLine = !info
    ? ''
    : !info.supported
      ? t('settings.hdrDisplay.unsupported')
      : info.active
        ? t('settings.hdrDisplay.active', {
            peak: Math.round(info.peakNits),
            white: Math.round(info.sdrWhiteNits),
            ev: info.headroomEv.toFixed(1),
          })
        : t('settings.hdrDisplay.inactive');

  return (
    <div className="space-y-8">
      <div>
        <Text variant={TextVariants.heading} className="block mb-2">
          {t('settings.hdrDisplay.mode')}
        </Text>
        <Dropdown
          onChange={(value: any) => update({ hdrDisplayMode: value })}
          options={[
            { value: 'auto', label: t('settings.hdrDisplay.modeAuto') },
            { value: 'on', label: t('settings.hdrDisplay.modeOn') },
            { value: 'off', label: t('settings.hdrDisplay.modeOff') },
          ]}
          value={mode}
          triggerClassName="bg-bg-primary"
        />
        <Text variant={TextVariants.small} className="mt-2">
          {t('settings.hdrDisplay.modeDesc')}
        </Text>
        {statusLine && (
          <Text variant={TextVariants.small} className="mt-2">
            {statusLine}
          </Text>
        )}
        <Text variant={TextVariants.small} className="mt-1">
          {detectedLine}
        </Text>
      </div>

      <div>
        <Text variant={TextVariants.heading} className="block mb-2">
          {t('settings.hdrDisplay.peak')}
        </Text>
        <Switch
          checked={peakOverride === null}
          id="hdr-peak-auto"
          label={t('settings.hdrDisplay.useDetected')}
          onChange={(auto) => update({ hdrPeakNits: auto ? null : peak })}
        />
        <Slider
          label={t('settings.hdrDisplay.nits')}
          min={PEAK_RANGE.min}
          max={PEAK_RANGE.max}
          step={PEAK_RANGE.step}
          value={peak}
          defaultValue={1000}
          disabled={peakOverride === null}
          onChange={(e: any) => update({ hdrPeakNits: parseFloat(e.target.value) })}
          fillOrigin="min"
        />
        <Text variant={TextVariants.small} className="mt-2">
          {t('settings.hdrDisplay.peakDesc')}
        </Text>
      </div>

      <div>
        <Text variant={TextVariants.heading} className="block mb-2">
          {t('settings.hdrDisplay.sdrWhite')}
        </Text>
        <Switch
          checked={whiteOverride === null}
          id="hdr-white-auto"
          label={t('settings.hdrDisplay.useDetected')}
          onChange={(auto) => update({ hdrSdrWhiteNits: auto ? null : white })}
        />
        <Slider
          label={t('settings.hdrDisplay.nits')}
          min={SDR_WHITE_RANGE.min}
          max={SDR_WHITE_RANGE.max}
          step={SDR_WHITE_RANGE.step}
          value={white}
          defaultValue={203}
          disabled={whiteOverride === null}
          onChange={(e: any) => update({ hdrSdrWhiteNits: parseFloat(e.target.value) })}
          fillOrigin="min"
        />
        <Text variant={TextVariants.small} className="mt-2">
          {t('settings.hdrDisplay.sdrWhiteDesc')}
        </Text>
      </div>
    </div>
  );
};

export default HdrDisplaySettings;
