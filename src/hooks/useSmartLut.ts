import { useCallback, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'react-toastify';
import { create } from 'zustand';
import { useEditorStore } from '../store/useEditorStore';
import { useEditorActions } from './useEditorActions';
import { Invokes } from '../types/invokes';
import { prepareAdjustmentsPayload } from '../utils/backendPayload';
import { getActiveTools, setToolActive } from '../utils/adjustments';
import { needsAnalysis, SmartLook, SmartLutAnalysis, SMART_LUT_NEUTRAL } from '../utils/smartLut';

interface SmartLutProgress {
  /** Path of the photo being analysed, if any. */
  analyzing: string | null;
  set(analyzing: string | null): void;
}

export const useSmartLutProgress = create<SmartLutProgress>((set) => ({
  analyzing: null,
  set: (analyzing) => set({ analyzing }),
}));

/** Applying looks and keeping the open photo's Smart LUT analysis current. */
export function useSmartLut() {
  const { setAdjustments } = useEditorActions();

  const analyze = useCallback(async () => {
    const { selectedImage, adjustments } = useEditorStore.getState();
    const path = selectedImage?.path;
    if (!path || !selectedImage?.isReady || useSmartLutProgress.getState().analyzing === path) return;
    useSmartLutProgress.getState().set(path);
    try {
      const analysis = await invoke<SmartLutAnalysis>(Invokes.AnalyzeSmartLutTarget, {
        jsAdjustments: prepareAdjustmentsPayload(adjustments).payload,
        path,
      });
      if (useEditorStore.getState().selectedImage?.path !== path) return;
      setAdjustments((prev) =>
        prev.smartLut ? { ...prev, smartLut: { ...prev.smartLut, analysis: { ...analysis, path } } } : prev,
      );
    } catch (error) {
      toast.error(`Smart LUT: ${error}`);
    } finally {
      if (useSmartLutProgress.getState().analyzing === path) useSmartLutProgress.getState().set(null);
    }
  }, [setAdjustments]);

  const applyLook = useCallback(
    (look: SmartLook) => {
      const path = useEditorStore.getState().selectedImage?.path;
      setAdjustments((prev) => {
        const analysis = prev.smartLut?.analysis?.path === path ? prev.smartLut?.analysis : null;
        return {
          ...prev,
          smartLut: { look, color: SMART_LUT_NEUTRAL, luminance: SMART_LUT_NEUTRAL, analysis: analysis ?? null },
          activeTools: setToolActive(getActiveTools(prev), 'smartLut', true),
        };
      }, 'Smart LUT');
    },
    [setAdjustments],
  );

  const clearLook = useCallback(() => {
    setAdjustments((prev) => ({ ...prev, smartLut: null }), 'Smart LUT');
  }, [setAdjustments]);

  return { analyze, applyLook, clearLook };
}

/** Reads the open photo whenever its look has no analysis for it (a new look, a pasted edit, another photo). */
export function useSmartLutAutoAnalysis() {
  const { analyze } = useSmartLut();
  const smartLut = useEditorStore((state) => state.adjustments.smartLut);
  const path = useEditorStore((state) => state.selectedImage?.path);
  const isReady = useEditorStore((state) => state.selectedImage?.isReady ?? false);

  useEffect(() => {
    if (isReady && needsAnalysis(smartLut, path)) {
      analyze();
    }
  }, [smartLut, path, isReady, analyze]);
}
