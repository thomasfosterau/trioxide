import { defineConfig } from 'vite';
import trioxide from 'trioxide/vite';

// `vite build --mode plain` builds without trioxide, for comparison.
export default defineConfig(({ mode }) => ({
  plugins: mode === 'plain' ? [] : [trioxide()],
}));
