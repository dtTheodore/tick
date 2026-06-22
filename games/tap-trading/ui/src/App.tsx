import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { Game } from './routes/Game';
import { LandingPage } from './landing/LandingPage';

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<LandingPage />} />
        <Route path="/play" element={<Game />} />
      </Routes>
    </BrowserRouter>
  );
}
