import { useEffect } from 'react'
import { BrowserRouter, Routes, Route, useNavigate, useLocation } from 'react-router-dom'
import { listen } from '@tauri-apps/api/event'
import './styles.css'
import { connectToNode } from './bindings'
import { HomeScreen } from './components/HomeScreen'
import { AcceptingConnScreen } from './components/AcceptingConnScreen'
import { ConnectingScreen } from './components/ConnectingScreen'
import { QrScannerScreen } from './components/QrScannerScreen'
import { AcceptedConnScreen } from './components/AcceptedConnScreen'
import { ErrorScreen } from './components/ErrorScreen'

function AppContent() {
  const navigate = useNavigate();
  const location = useLocation();

  useEffect(() => {
    // Listen for connection accepted event
    const unlisten = listen('connection-accepted', () => {
      navigate('/connected');
    });

    return () => {
      unlisten.then(fn => fn());
    };
  }, [navigate]);

  const handleConnect = async (nodeId: string) => {
    try {
      await connectToNode(nodeId);
      navigate('/connected');
    } catch (err) {
      console.error('Failed to connect:', err);
      navigate('/error', {
        state: {
          message: 'Failed to connect to node',
          details: err instanceof Error ? err.message : String(err)
        }
      });
    }
  };

  return (
    <Routes>
      <Route path="/" element={<HomeScreen />} />
      <Route 
        path="/accepting" 
        element={<AcceptingConnScreen 
          onBack={() => navigate('/')}
          nodeId={(location.state as { nodeId: string })?.nodeId || ''}
        />} 
      />
      <Route 
        path="/connecting" 
        element={<ConnectingScreen 
          onBack={() => navigate('/')}
          onScanClick={() => navigate('/scanning')}
          onConnect={handleConnect}
        />}
      />
      <Route 
        path="/scanning" 
        element={<QrScannerScreen 
          onBack={() => navigate('/connecting')}
          onScan={handleConnect}
        />}
      />
      <Route 
        path="/connected" 
        element={<AcceptedConnScreen 
          onBack={() => navigate('/')}
        />}
      />
      <Route path="/error" element={<ErrorScreen />} />
    </Routes>
  );
}

function App() {
  return (
    <BrowserRouter>
      <AppContent />
    </BrowserRouter>
  );
}

export default App
