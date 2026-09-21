import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { RouterProvider } from 'react-router-dom';

import { createAppRouter } from '@/app/routes';
import '@/styles/globals.css';

const container = document.getElementById('root');

if (!container) {
  throw new Error('找不到 #root 挂载点，index.html 可能被改动过。');
}

createRoot(container).render(
  <StrictMode>
    <RouterProvider router={createAppRouter()} />
  </StrictMode>,
);
