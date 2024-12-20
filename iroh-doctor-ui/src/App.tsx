import { useEffect } from 'react'
import { BrowserRouter, Routes, Route, useNavigate, useLocation } from 'react-router-dom'
import { listen } from '@tauri-apps/api/event'
import './styles.css'
import { startAcceptingConnections, connectToNode } from './bindings'
import { AcceptingConnScreen } from './components/AcceptingConnScreen'
import { ConnectingScreen } from './components/ConnectingScreen'
import { QrScannerScreen } from './components/QrScannerScreen'
import { AcceptedConnScreen } from './components/AcceptedConnScreen'
import { ScreenWrapper } from './components/ScreenWrapper'

function HomeScreen() {
  const navigate = useNavigate();

  const handleAcceptConnections = async () => {
    try {
      const nodeId = await startAcceptingConnections();
      navigate('/accepting', { state: { nodeId } });
    } catch (err) {
      console.error('Failed to start accepting connections:', err);
      // TODO: Show error to user
    }
  };

  return (
    <ScreenWrapper>
      <div className="flex-col space-y-3">
        <button 
          className="w-full my-4 p-3 px-4 transition bg-irohGray-800 text-irohPurple-500 uppercase hover:bg-irohGray-700 hover:text-gray-200 font-medium"
          onClick={handleAcceptConnections}
        >
          Accept Connections
        </button>
        
        <button 
          className="w-full my-4 p-3 px-4 transition bg-white text-irohGray-800 uppercase hover:bg-irohGray-100 border border-irohGray-200 font-medium"
          onClick={() => navigate('/connecting')}
        >
          Connect
        </button>
      </div>
    </ScreenWrapper>
  );
}

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
      // TODO: Show error to user
      navigate('/');
    }
  };

  return (
    <Routes>
      <Route path="/" element={<HomeScreen />} />
      <Route 
        path="/accepting" 
        element={<AcceptingConnScreen 
          nodeId={(location.state as { nodeId: string })?.nodeId || ''}
          onBack={() => navigate('/')}
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
