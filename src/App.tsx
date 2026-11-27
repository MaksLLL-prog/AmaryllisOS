import React, { useState } from 'react';
import { Desktop } from './components/Desktop';
import { TextConsole } from './components/TextConsole';

export const App: React.FC = () => {
  const [guiMode, setGuiMode] = useState(true);

  if (!guiMode) {
    return <TextConsole onLaunchGui={() => setGuiMode(true)} />;
  }

  return <Desktop onExitDesktop={() => setGuiMode(false)} />;
};

export default App;
