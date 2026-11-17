import { usePhotoPalette } from '../../hooks/usePhotoPalette';

/** Isolates the palette hook (which subscribes to every adjustment change) so it never re-renders the whole App. */
export default function PhotoPalette() {
  usePhotoPalette();
  return null;
}
