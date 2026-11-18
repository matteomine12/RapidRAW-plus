import { useCallback, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'react-toastify';
import { create } from 'zustand';
import { useEditorStore } from '../store/useEditorStore';
import { useEditorActions } from './useEditorActions';
import { Invokes } from '../types/invokes';
import { prepareAdjustmentsPayload } from '../utils/backendPayload';
import { getActiveTools, setToolActive } from '../utils/adjustments';
import { needsAnalysis, SmartLook, SmartLutAnalysis, SmartLutEngine, SMART_LUT_NEUTRAL } from '../utils/smartLut';

interface SmartLutProgress {
  /** Path of the photo being analysed, if any. */
  analyzing: string | null;
  /** The neural engine is running. */
  neuralRunning: boolean;
  set(analyzing: string | null): void;
  setNeural(running: boolean): void;
}

export const useSmartLutProgress = create<SmartLutProgress>((set) => ({
  analyzing: null,
  neuralRunning: false,
  set: (analyzing) => set({ analyzing }),
  setNeural: (neuralRunning) => set({ neuralRunning }),
}));

/** Neural LUTs the backend holds in this session (look id : analysis id). */
const neuralReady = new Set<string>();
const neuralKey = (lookId: string, analysisId: string) => `${lookId}:${analysisId}`;

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

  /** Runs the neural engine for the current look on the open photo, then lets the preview use it. */
  const prepareNeural = useCallback(async () => {
    const { adjustments, selectedImage } = useEditorStore.getState();
    const smart = adjustments.smartLut;
    const analysisId = smart?.analysis?.id;
    if (!smart || !analysisId || !selectedImage?.path || useSmartLutProgress.getState().neuralRunning) return;
    const key = neuralKey(smart.look.id, analysisId);
    useSmartLutProgress.getState().setNeural(true);
    try {
      await invoke(Invokes.PrepareSmartLutNeural, {
        jsAdjustments: prepareAdjustmentsPayload(adjustments).payload,
        lookId: smart.look.id,
        analysisId,
      });
      neuralReady.add(key);
      setAdjustments((prev) =>
        prev.smartLut ? { ...prev, smartLut: { ...prev.smartLut, neuralStamp: Date.now() } } : prev,
      );
    } catch (error) {
      toast.error(`Smart LUT: ${error}`);
      setAdjustments((prev) =>
        prev.smartLut ? { ...prev, smartLut: { ...prev.smartLut, engine: 'analytic' } } : prev,
      );
    } finally {
      useSmartLutProgress.getState().setNeural(false);
    }
  }, [setAdjustments]);

  const setEngine = useCallback(
    (engine: SmartLutEngine) => {
      setAdjustments(
        (prev) => (prev.smartLut ? { ...prev, smartLut: { ...prev.smartLut, engine } } : prev),
        'Smart LUT',
      );
    },
    [setAdjustments],
  );

  const applyLook = useCallback(
    (look: SmartLook) => {
      const path = useEditorStore.getState().selectedImage?.path;
      setAdjustments((prev) => {
        const analysis = prev.smartLut?.analysis?.path === path ? prev.smartLut?.analysis : null;
        return {
          ...prev,
          smartLut: {
            look,
            color: SMART_LUT_NEUTRAL,
            luminance: SMART_LUT_NEUTRAL,
            analysis: analysis ?? null,
            engine: look.hasReference ? (prev.smartLut?.engine ?? 'analytic') : 'analytic',
          },
          activeTools: setToolActive(getActiveTools(prev), 'smartLut', true),
        };
      }, 'Smart LUT');
    },
    [setAdjustments],
  );

  const clearLook = useCallback(() => {
    setAdjustments((prev) => ({ ...prev, smartLut: null }), 'Smart LUT');
  }, [setAdjustments]);

  return { analyze, applyLook, clearLook, prepareNeural, setEngine };
}

/** Reads the open photo whenever its look has no analysis for it (a new look, a pasted edit, another photo). */
export function useSmartLutAutoAnalysis() {
  const { analyze, prepareNeural } = useSmartLut();
  const smartLut = useEditorStore((state) => state.adjustments.smartLut);
  const path = useEditorStore((state) => state.selectedImage?.path);
  const isReady = useEditorStore((state) => state.selectedImage?.isReady ?? false);

  useEffect(() => {
    if (isReady && needsAnalysis(smartLut, path)) {
      analyze();
    }
  }, [smartLut, path, isReady, analyze]);

  // the neural engine runs once per look and analysed photo in a session
  const lookId = smartLut?.look.id;
  const analysisId = smartLut?.analysis?.path === path ? smartLut?.analysis?.id : undefined;
  const wantsNeural = smartLut?.engine === 'neural';
  useEffect(() => {
    if (isReady && wantsNeural && lookId && analysisId && !neuralReady.has(neuralKey(lookId, analysisId))) {
      prepareNeural();
    }
  }, [isReady, wantsNeural, lookId, analysisId, prepareNeural]);
}
