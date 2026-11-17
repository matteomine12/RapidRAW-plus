import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export interface HdrDisplayInfo {
  supported: boolean;
  active: boolean;
  peakNits: number;
  sdrWhiteNits: number;
  headroomEv: number;
}

/** The display's HDR state (peak and SDR white nits), kept current when HDR is switched on or off. */
export function useHdrDisplayInfo(): HdrDisplayInfo | null {
  const [info, setInfo] = useState<HdrDisplayInfo | null>(null);

  useEffect(() => {
    let disposed = false;
    invoke<HdrDisplayInfo>('get_hdr_display_info')
      .then((next) => !disposed && setInfo(next))
      .catch(() => {});
    const unlisten = listen<HdrDisplayInfo>('hdr-display-changed', (event) => setInfo(event.payload));
    return () => {
      disposed = true;
      unlisten.then((fn) => fn());
    };
  }, []);

  return info;
}

export const formatNits = (nits: number) =>
  nits >= 1000 ? `${(nits / 1000).toFixed(nits >= 10000 ? 0 : 1).replace(/\.0$/, '')}k` : `${Math.round(nits)}`;
