import { useNavigate } from 'react-router-dom';
import { startAcceptingConnections } from '../bindings';
import { ScreenWrapper } from './ScreenWrapper';

export function HomeScreen() {
  const navigate = useNavigate();

  const handleAcceptConnections = async () => {
    try {
      const nodeId = await startAcceptingConnections();
      navigate('/accepting', { state: { nodeId } });
    } catch (err) {
      console.error('Failed to start accepting connections:', err);
      navigate('/error', {
        state: {
          message: 'Failed to start accepting connections',
          details: err instanceof Error ? err.message : String(err)
        }
      });
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