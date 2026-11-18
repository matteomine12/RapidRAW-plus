import { memo, useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { AnimatePresence, motion } from 'framer-motion';
import clsx from 'clsx';
import { ImagePlus, Loader2, Palette, Pencil, Plus, Trash2, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { toast } from 'react-toastify';
import SmartLutPad from './SmartLutPad';
import { useContextMenu } from '../../context/ContextMenuContext';
import { useEditorStore } from '../../store/useEditorStore';
import { selectAiModelDownloadStatus, useProcessStore } from '../../store/useProcessStore';
import { useSmartLut, useSmartLutProgress } from '../../hooks/useSmartLut';
import { Invokes } from '../../types/invokes';
import { Adjustments, areSectionPropsEqual } from '../../utils/adjustments';
import { SPRING } from '../../utils/motion';
import { hexToRgb, lookGradient, SmartLook, SmartLutEngine } from '../../utils/smartLut';

const REFERENCE_EXTENSIONS = [
  'jpg',
  'jpeg',
  'png',
  'tif',
  'tiff',
  'webp',
  'heic',
  'avif',
  'arw',
  'cr2',
  'cr3',
  'nef',
  'dng',
  'raf',
  'orf',
  'rw2',
  'pef',
];
const MAX_COLORS = 5;
const DEFAULT_COLORS = ['#2a9d8f', '#e9c46a'];

interface SmartLutPanelProps {
  adjustments: Adjustments;
  setAdjustments(value: Partial<Adjustments> | ((prev: Adjustments) => Adjustments), label?: string): void;
  onDragStateChange?: (isDragging: boolean) => void;
}

function LookTile({
  look,
  selected,
  renaming,
  onSelect,
  onContextMenu,
  onRename,
}: {
  look: SmartLook;
  selected: boolean;
  renaming: boolean;
  onSelect(): void;
  onContextMenu(e: React.MouseEvent): void;
  onRename(name: string | null): void;
}) {
  const [name, setName] = useState(look.name);
  useEffect(() => setName(look.name), [look.name, renaming]);

  return (
    <div className="flex flex-col gap-1 min-w-0">
      <motion.button
        onClick={onSelect}
        onContextMenu={onContextMenu}
        whileTap={{ scale: 0.94 }}
        transition={SPRING.snappy}
        className={clsx(
          'relative aspect-square rounded-2xl overflow-hidden outline-none ring-offset-2 ring-offset-bg-secondary transition-shadow',
          selected ? 'ring-2 ring-accent' : 'hover:ring-2 hover:ring-surface',
        )}
        style={{ background: lookGradient(look) }}
        data-tooltip={look.name}
      >
        <span
          className="absolute inset-0 opacity-50"
          style={{
            backgroundImage: 'radial-gradient(circle, rgba(255,255,255,0.85) 1px, transparent 1.4px)',
            backgroundSize: '20% 20%',
            backgroundPosition: '10% 10%',
          }}
        />
      </motion.button>
      {renaming ? (
        <input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => onRename(name)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') onRename(name);
            if (e.key === 'Escape') onRename(null);
          }}
          className="w-full text-[11px] bg-surface rounded-md px-1 py-0.5 text-text-primary outline-none"
        />
      ) : (
        <span className="text-[11px] text-text-secondary truncate text-center select-none">{look.name}</span>
      )}
    </div>
  );
}

/** Analytic (the hand-built engine) or neural (the trained model, beta), to compare the two. */
function EngineSwitch({
  engine,
  neuralAvailable,
  running,
  onChange,
}: {
  engine: SmartLutEngine;
  neuralAvailable: boolean;
  running: boolean;
  onChange(engine: SmartLutEngine): void;
}) {
  const { t } = useTranslation();
  const options: Array<{ id: SmartLutEngine; label: string; disabled: boolean; tooltip?: string }> = [
    { id: 'analytic', label: t('adjustments.smartLut.engineAnalytic'), disabled: false },
    {
      id: 'neural',
      label: t('adjustments.smartLut.engineNeural'),
      disabled: !neuralAvailable,
      tooltip: neuralAvailable
        ? t('adjustments.smartLut.engineNeuralHint')
        : t('adjustments.smartLut.engineNeuralMissing'),
    },
  ];
  return (
    <div className="mt-3 mx-auto w-full max-w-[260px] flex p-1 rounded-full bg-surface">
      {options.map((option) => (
        <button
          key={option.id}
          disabled={option.disabled}
          onClick={() => onChange(option.id)}
          data-tooltip={option.tooltip}
          className={clsx(
            'relative flex-1 flex items-center justify-center gap-1.5 py-1.5 rounded-full text-xs font-medium transition-colors disabled:opacity-40 disabled:cursor-not-allowed',
            engine === option.id ? 'text-button-text' : 'text-text-secondary hover:text-text-primary',
          )}
        >
          {engine === option.id && (
            <motion.span
              layoutId="smart-lut-engine"
              className="absolute inset-0 rounded-full bg-accent"
              transition={SPRING.bouncy}
            />
          )}
          <span className="relative flex items-center gap-1.5">
            {option.id === 'neural' && running && <Loader2 size={12} className="animate-spin" />}
            {option.label}
          </span>
        </button>
      ))}
    </div>
  );
}

function ColorLookCreator({
  onCreate,
  onCancel,
}: {
  onCreate(colors: string[], name: string): void;
  onCancel(): void;
}) {
  const { t } = useTranslation();
  const [colors, setColors] = useState<string[]>(DEFAULT_COLORS);
  const [name, setName] = useState('');
  const preview = {
    palette: colors.map((c) => ({ rgb: hexToRgb(c), weight: 1 / colors.length })),
    monochrome: false,
  };

  return (
    <motion.div
      initial={{ height: 0, opacity: 0 }}
      animate={{ height: 'auto', opacity: 1 }}
      exit={{ height: 0, opacity: 0 }}
      transition={SPRING.standard}
      className="overflow-hidden"
    >
      <div className="mt-3 p-3 rounded-2xl bg-surface flex flex-col gap-3">
        <div className="flex items-center gap-3">
          <div className="w-14 h-14 shrink-0 rounded-xl" style={{ background: lookGradient(preview) }} />
          <div className="flex flex-wrap items-center gap-2">
            {colors.map((c, i) => (
              <label
                key={i}
                className="relative w-8 h-8 rounded-full cursor-pointer ring-1 ring-white/20 shadow-sm"
                style={{ background: c }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  if (colors.length > 1) setColors(colors.filter((_, j) => j !== i));
                }}
                data-tooltip={t('adjustments.smartLut.colorTooltip')}
              >
                <input
                  type="color"
                  value={c}
                  onChange={(e) => setColors(colors.map((v, j) => (j === i ? e.target.value : v)))}
                  className="absolute inset-0 opacity-0 cursor-pointer"
                />
              </label>
            ))}
            {colors.length < MAX_COLORS && (
              <button
                onClick={() => setColors([...colors, colors[colors.length - 1]])}
                className="w-8 h-8 rounded-full border-2 border-dashed border-text-secondary/30 hover:border-accent flex items-center justify-center text-text-secondary"
                data-tooltip={t('adjustments.smartLut.addColor')}
              >
                <Plus size={14} />
              </button>
            )}
          </div>
        </div>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t('adjustments.smartLut.namePlaceholder')}
          className="w-full text-sm bg-bg-secondary rounded-lg px-3 py-2 text-text-primary outline-none focus:ring-2 focus:ring-accent"
        />
        <div className="flex justify-end gap-2">
          <button
            onClick={onCancel}
            className="px-4 py-1.5 rounded-full text-sm text-text-secondary hover:bg-bg-secondary"
          >
            {t('adjustments.smartLut.cancel')}
          </button>
          <button
            onClick={() => onCreate(colors, name)}
            className="px-4 py-1.5 rounded-full text-sm font-semibold bg-accent text-button-text"
          >
            {t('adjustments.smartLut.create')}
          </button>
        </div>
      </div>
    </motion.div>
  );
}

function SmartLutPanel({ adjustments, setAdjustments, onDragStateChange }: SmartLutPanelProps) {
  const { t } = useTranslation();
  const { showContextMenu } = useContextMenu();
  const { applyLook, clearLook, setEngine } = useSmartLut();
  const neuralRunning = useSmartLutProgress((state) => state.neuralRunning);
  const path = useEditorStore((state) => state.selectedImage?.path);
  const analyzing = useSmartLutProgress((state) => state.analyzing);
  const downloadStatus = useProcessStore(selectAiModelDownloadStatus);
  const [looks, setLooks] = useState<SmartLook[]>([]);
  const [creatingFromImage, setCreatingFromImage] = useState(false);
  const [showColorCreator, setShowColorCreator] = useState(false);
  const [renamingId, setRenamingId] = useState<string | null>(null);

  const smartLut = adjustments.smartLut ?? null;
  const isAnalyzing = !!path && analyzing === path;
  const isBusy = isAnalyzing || creatingFromImage;

  const refresh = useCallback(async () => {
    try {
      setLooks(await invoke<SmartLook[]>(Invokes.ListSmartLuts));
    } catch (error) {
      console.error('Failed to list Smart LUTs:', error);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const createFromImage = async () => {
    const selected = await open({
      multiple: false,
      filters: [
        {
          name: t('adjustments.smartLut.imagesFilter'),
          extensions: [...REFERENCE_EXTENSIONS, ...REFERENCE_EXTENSIONS.map((e) => e.toUpperCase())],
        },
      ],
    });
    if (typeof selected !== 'string') return;
    setCreatingFromImage(true);
    try {
      const look = await invoke<SmartLook>(Invokes.CreateSmartLutFromImage, { path: selected, name: null });
      setLooks((prev) => [look, ...prev]);
      applyLook(look);
    } catch (error) {
      toast.error(`Smart LUT: ${error}`);
    } finally {
      setCreatingFromImage(false);
    }
  };

  const createFromColors = async (colors: string[], name: string) => {
    try {
      const look = await invoke<SmartLook>(Invokes.CreateSmartLutFromColors, {
        colors: colors.map(hexToRgb),
        name: name.trim() || (colors.length === 1 ? colors[0] : t('adjustments.smartLut.paletteName')),
      });
      setLooks((prev) => [look, ...prev]);
      setShowColorCreator(false);
      applyLook(look);
    } catch (error) {
      toast.error(`Smart LUT: ${error}`);
    }
  };

  const handleAddMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    showContextMenu(e.clientX, e.clientY, [
      { label: t('adjustments.smartLut.fromImage'), icon: ImagePlus, onClick: createFromImage },
      { label: t('adjustments.smartLut.fromColors'), icon: Palette, onClick: () => setShowColorCreator(true) },
    ]);
  };

  const handleTileMenu = (e: React.MouseEvent, look: SmartLook) => {
    e.preventDefault();
    e.stopPropagation();
    showContextMenu(e.clientX, e.clientY, [
      { label: t('adjustments.smartLut.rename'), icon: Pencil, onClick: () => setRenamingId(look.id) },
      {
        label: t('adjustments.smartLut.delete'),
        icon: Trash2,
        isDestructive: true,
        onClick: async () => {
          try {
            await invoke(Invokes.DeleteSmartLut, { id: look.id });
            setLooks((prev) => prev.filter((l) => l.id !== look.id));
          } catch (error) {
            toast.error(`Smart LUT: ${error}`);
          }
        },
      },
    ]);
  };

  const handleRename = async (look: SmartLook, name: string | null) => {
    setRenamingId(null);
    if (name === null || !name.trim() || name.trim() === look.name) return;
    try {
      const renamed = await invoke<SmartLook>(Invokes.RenameSmartLut, { id: look.id, name });
      setLooks((prev) => prev.map((l) => (l.id === look.id ? renamed : l)));
      if (smartLut?.look.id === look.id) {
        setAdjustments((prev) =>
          prev.smartLut
            ? { ...prev, smartLut: { ...prev.smartLut, look: { ...prev.smartLut.look, name: renamed.name } } }
            : prev,
        );
      }
    } catch (error) {
      toast.error(`Smart LUT: ${error}`);
    }
  };

  const handlePadChange = useCallback(
    (color: number, luminance: number) =>
      setAdjustments((prev) => (prev.smartLut ? { ...prev, smartLut: { ...prev.smartLut, color, luminance } } : prev)),
    [setAdjustments],
  );

  return (
    <div className="flex flex-col gap-3">
      <AnimatePresence initial={false}>
        {smartLut && (
          <motion.div
            key="pad"
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={SPRING.standard}
            className="overflow-hidden"
          >
            <div className="flex items-center justify-between mb-2">
              <span className="text-sm font-medium text-text-primary truncate">{smartLut.look.name}</span>
              <button
                onClick={clearLook}
                className="p-1 rounded-full hover:bg-surface text-text-secondary hover:text-text-primary transition-colors"
                data-tooltip={t('adjustments.smartLut.remove')}
              >
                <X size={14} />
              </button>
            </div>
            <div className="relative">
              <SmartLutPad
                look={smartLut.look}
                color={smartLut.color}
                luminance={smartLut.luminance}
                disabled={isAnalyzing}
                colorLabel={t('adjustments.smartLut.color')}
                luminanceLabel={t('adjustments.smartLut.luminance')}
                onChange={handlePadChange}
                onDragStateChange={onDragStateChange}
              />
              <AnimatePresence>
                {isAnalyzing && (
                  <motion.div
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    exit={{ opacity: 0 }}
                    className="absolute inset-x-0 top-0 aspect-square max-w-[260px] mx-auto flex flex-col items-center justify-center gap-1 rounded-[28px] bg-black/45 text-white text-xs text-center px-6 pointer-events-none"
                  >
                    <Loader2 size={18} className="animate-spin" />
                    <span>
                      {downloadStatus
                        ? t('editor.masks.settings.aiModelDownloading')
                        : t('adjustments.smartLut.analyzing')}
                    </span>
                    {downloadStatus && <span className="opacity-80">{downloadStatus}</span>}
                  </motion.div>
                )}
              </AnimatePresence>
            </div>
            <EngineSwitch
              engine={smartLut.engine ?? 'analytic'}
              neuralAvailable={!!smartLut.look.hasReference}
              running={neuralRunning}
              onChange={setEngine}
            />
          </motion.div>
        )}
      </AnimatePresence>

      <div>
        <span className="text-sm font-medium text-text-secondary select-none block mb-2">
          {t('adjustments.smartLut.library')}
        </span>
        <div className="grid grid-cols-4 gap-2">
          <motion.button
            onClick={handleAddMenu}
            onContextMenu={handleAddMenu}
            disabled={isBusy}
            whileTap={{ scale: 0.94 }}
            transition={SPRING.snappy}
            className="aspect-square rounded-2xl border-2 border-dashed border-text-secondary/25 hover:border-accent flex items-center justify-center text-text-secondary hover:text-text-primary transition-colors disabled:opacity-50"
            data-tooltip={t('adjustments.smartLut.create')}
          >
            {creatingFromImage ? <Loader2 size={18} className="animate-spin" /> : <Plus size={18} />}
          </motion.button>
          {looks.map((look) => (
            <LookTile
              key={look.id}
              look={look}
              selected={smartLut?.look.id === look.id}
              renaming={renamingId === look.id}
              onSelect={() => (smartLut?.look.id === look.id ? clearLook() : applyLook(look))}
              onContextMenu={(e) => handleTileMenu(e, look)}
              onRename={(name) => handleRename(look, name)}
            />
          ))}
        </div>
        {looks.length === 0 && !showColorCreator && (
          <p className="mt-2 text-xs text-text-secondary">{t('adjustments.smartLut.empty')}</p>
        )}
        <AnimatePresence>
          {showColorCreator && (
            <ColorLookCreator key="creator" onCreate={createFromColors} onCancel={() => setShowColorCreator(false)} />
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}

export default memo(SmartLutPanel, areSectionPropsEqual('smartLut'));
