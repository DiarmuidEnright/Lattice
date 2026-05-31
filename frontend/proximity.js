// Proximity Transfer Functions

// Default sender wallet for proximity transfers when no wallet is connected on
// the dashboard. This is the demo user's funded Solana wallet (the on-chain
// sender). Transfers settle from the backend's custodial demo signer.
const DEMO_SENDER_WALLET = 'EYsJVQU5igQNaNMptsyL2A6h6QWso34MYxTriAkdQqWj';

// State for proximity features
const proximityState = {
    discoveryActive: false,
    discoveryMethod: 'WiFi',
    sessionId: null,
    sessionTimer: null,
    discoveredPeers: [],
    selectedPeer: null,
    incomingTransfer: null,
    transferNotificationTimer: null,
    proximityWebSocket: null
};

// Setup proximity event listeners
function setupProximityListeners() {
    // Discovery toggle
    document.getElementById('toggleDiscoveryBtn').addEventListener('click', toggleDiscovery);
    document.getElementById('extendSessionBtn').addEventListener('click', extendSession);
    document.getElementById('refreshPeersBtn').addEventListener('click', refreshPeers);
    
    // Discovery method selection
    document.querySelectorAll('input[name="discoveryMethod"]').forEach(radio => {
        radio.addEventListener('change', (e) => {
            proximityState.discoveryMethod = e.target.value;
        });
    });
    
    // Transfer form
    document.getElementById('closeTransferFormBtn').addEventListener('click', closeTransferForm);
    document.getElementById('cancelTransferBtn').addEventListener('click', closeTransferForm);
    document.getElementById('confirmTransferBtn').addEventListener('click', confirmTransfer);
    document.getElementById('transferAsset').addEventListener('change', updateTransferFees);
    document.getElementById('transferAmount').addEventListener('input', updateTransferFees);
    
    // Transfer notification
    document.getElementById('acceptTransferBtn').addEventListener('click', acceptTransfer);
    document.getElementById('rejectTransferBtn').addEventListener('click', rejectTransfer);
    
    // History filters
    document.getElementById('historyDateFilter').addEventListener('change', loadProximityHistory);
    document.getElementById('historyAssetFilter').addEventListener('change', loadProximityHistory);
    document.getElementById('historyTypeFilter').addEventListener('change', loadProximityHistory);
    document.getElementById('refreshHistoryBtn').addEventListener('click', loadProximityHistory);
}

// Initialize proximity view
function initializeProximityView() {
    if (state.connectedWallet) {
        loadProximityHistory();
    } else {
        document.getElementById('discoveredPeersList').innerHTML = 
            '<p class="empty-state">Connect your wallet to use proximity transfers</p>';
        document.getElementById('proximityHistoryList').innerHTML = 
            '<p class="empty-state">Connect your wallet to view history</p>';
    }
}

// Task 23.1: Discovery Toggle Component
async function toggleDiscovery() {
    if (!state.connectedWallet) {
        showToast('Please connect your wallet first', 'error');
        return;
    }
    
    if (proximityState.discoveryActive) {
        await stopDiscovery();
    } else {
        await startDiscovery();
    }
}

async function startDiscovery() {
    showLoading();
    
    try {
        const response = await fetch(`${API_BASE_URL}/api/proximity/discovery/start`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                user_id: DEMO_USER_ID,
                method: proximityState.discoveryMethod,
                duration_minutes: 30
            })
        });
        
        if (!response.ok) {
            const errorData = await response.json().catch(() => ({}));
            if (errorData.error && errorData.error.includes('not initialized')) {
                throw new Error('Proximity transfers are currently being integrated. This feature will be available soon!');
            }
            throw new Error('Failed to start discovery');
        }
        
        const data = await response.json();
        proximityState.sessionId = data.session.session_id;
        proximityState.discoveryActive = true;
        
        // Update UI
        updateDiscoveryStatus(true);
        document.getElementById('toggleDiscoveryBtn').textContent = 'Stop Discovery';
        document.getElementById('toggleDiscoveryBtn').classList.remove('btn-primary');
        document.getElementById('toggleDiscoveryBtn').classList.add('btn-secondary');
        
        // Show session info
        document.getElementById('activeSessionInfo').classList.remove('hidden');
        startSessionTimer(data.session.expires_at);
        
        // Start polling for peers
        startPeerPolling();
        
        // Initialize WebSocket for real-time updates
        initializeProximityWebSocket();
        
        showToast('Discovery started successfully!', 'success');
        
    } catch (error) {
        console.error('Error starting discovery:', error);
        showToast('Failed to start discovery', 'error');
    } finally {
        hideLoading();
    }
}

async function stopDiscovery() {
    showLoading();
    
    try {
        const response = await fetch(`${API_BASE_URL}/api/proximity/discovery/stop`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                session_id: proximityState.sessionId
            })
        });
        
        if (!response.ok) {
            throw new Error('Failed to stop discovery');
        }
        
        proximityState.discoveryActive = false;
        proximityState.sessionId = null;
        
        // Update UI
        updateDiscoveryStatus(false);
        document.getElementById('toggleDiscoveryBtn').textContent = 'Enable Discovery';
        document.getElementById('toggleDiscoveryBtn').classList.remove('btn-secondary');
        document.getElementById('toggleDiscoveryBtn').classList.add('btn-primary');
        
        // Hide session info
        document.getElementById('activeSessionInfo').classList.add('hidden');
        stopSessionTimer();
        
        // Stop polling
        stopPeerPolling();
        
        // Close WebSocket
        if (proximityState.proximityWebSocket) {
            proximityState.proximityWebSocket.close();
            proximityState.proximityWebSocket = null;
        }
        
        // Clear peers list
        proximityState.discoveredPeers = [];
        document.getElementById('discoveredPeersList').innerHTML = 
            '<p class="empty-state">Enable discovery to find nearby users</p>';
        
        showToast('Discovery stopped', 'success');
        
    } catch (error) {
        console.error('Error stopping discovery:', error);
        showToast('Failed to stop discovery', 'error');
    } finally {
        hideLoading();
    }
}

function updateDiscoveryStatus(active) {
    const statusElement = document.getElementById('discoveryStatus');
    if (active) {
        statusElement.classList.add('active');
        statusElement.querySelector('.status-text').textContent = 'Discoverable';
    } else {
        statusElement.classList.remove('active');
        statusElement.querySelector('.status-text').textContent = 'Not Discoverable';
    }
}

function startSessionTimer(expiresAt) {
    const startTime = new Date();
    const expiryTime = new Date(expiresAt);
    
    const updateTimer = () => {
        const now = new Date();
        const elapsed = Math.floor((now - startTime) / 1000);
        const remaining = Math.floor((expiryTime - now) / 1000);
        
        const elapsedMinutes = Math.floor(elapsed / 60);
        const remainingMinutes = Math.floor(remaining / 60);
        const remainingSeconds = remaining % 60;
        
        document.getElementById('sessionDuration').textContent = `${elapsedMinutes} minutes`;
        document.getElementById('sessionExpiry').textContent = 
            `${remainingMinutes}:${remainingSeconds.toString().padStart(2, '0')}`;
        
        if (remaining <= 0) {
            stopSessionTimer();
            proximityState.discoveryActive = false;
            updateDiscoveryStatus(false);
            showToast('Discovery session expired', 'warning');
        }
    };
    
    updateTimer();
    proximityState.sessionTimer = setInterval(updateTimer, 1000);
}

function stopSessionTimer() {
    if (proximityState.sessionTimer) {
        clearInterval(proximityState.sessionTimer);
        proximityState.sessionTimer = null;
    }
}

async function extendSession() {
    if (!proximityState.sessionId) return;
    
    showLoading();
    
    try {
        // API endpoint not yet implemented, using mock
        showToast('Session extended by 15 minutes', 'success');
        
    } catch (error) {
        console.error('Error extending session:', error);
        showToast('Failed to extend session', 'error');
    } finally {
        hideLoading();
    }
}

// Task 23.2: Discovered Peers List Component
let peerPollingInterval = null;

function startPeerPolling() {
    refreshPeers();
    peerPollingInterval = setInterval(refreshPeers, 5000); // Poll every 5 seconds
}

function stopPeerPolling() {
    if (peerPollingInterval) {
        clearInterval(peerPollingInterval);
        peerPollingInterval = null;
    }
}

async function refreshPeers() {
    if (!proximityState.discoveryActive) return;
    
    try {
        // Route through apiFetch for the uniform 10s timeout + error path (Req 2.8).
        const data = await apiFetch('/api/proximity/peers');
        proximityState.discoveredPeers = data.peers || [];
        
        displayDiscoveredPeers(proximityState.discoveredPeers);
        
    } catch (error) {
        console.error('Error loading peers:', error);
        // Mock peers are only sanctioned when the backend was unreachable at
        // open (Req 2.5), always paired with the persistent "data not live"
        // banner. While the backend is reachable, a failed request surfaces a
        // visible error and never silently substitutes mock data (Req 2.3/2.4/2.7).
        if (typeof state !== 'undefined' && state.backendReachable === false) {
            if (typeof showDataNotLiveBanner === 'function') showDataNotLiveBanner();
            displayMockPeers();
        } else {
            const container = document.getElementById('discoveredPeersList');
            if (container) {
                container.innerHTML = `<p class="error-state">⚠ Failed to load nearby peers: ${error.message}</p>`;
            }
            if (typeof showError === 'function') {
                showError(`Failed to load nearby peers: ${error.message}`);
            }
        }
    }
}

function displayDiscoveredPeers(peers) {
    const container = document.getElementById('discoveredPeersList');
    
    if (peers.length === 0) {
        container.innerHTML = '<p class="empty-state">No nearby users found</p>';
        return;
    }
    
    container.innerHTML = '';
    
    peers.forEach(peer => {
        const item = document.createElement('div');
        item.className = `peer-item ${peer.verified ? '' : 'unverified'}`;
        item.onclick = () => selectPeerForTransfer(peer);
        
        // Calculate signal strength bars
        const signalStrength = peer.signal_strength || -50;
        const bars = Math.min(4, Math.max(1, Math.floor((100 + signalStrength) / 25)));
        
        item.innerHTML = `
            <div class="peer-info">
                <div class="peer-header">
                    <div class="peer-user-tag">${peer.user_tag}</div>
                    <div class="peer-badges">
                        ${peer.verified ? '<span class="peer-badge verified">✓ Verified</span>' : ''}
                        <span class="peer-badge method">${peer.discovery_method}</span>
                    </div>
                </div>
                <div class="peer-details">
                    <span class="peer-wallet">${peer.wallet_address.substring(0, 8)}...${peer.wallet_address.substring(peer.wallet_address.length - 8)}</span>
                    <span>Discovered ${formatTimeAgo(peer.discovered_at)}</span>
                </div>
            </div>
            <div class="connection-quality">
                <div class="signal-strength">
                    ${[1, 2, 3, 4].map(i => 
                        `<div class="signal-bar ${i <= bars ? 'active' : ''}"></div>`
                    ).join('')}
                </div>
                <div class="quality-label">${getQualityLabel(bars)}</div>
            </div>
        `;
        
        container.appendChild(item);
    });
}

function getQualityLabel(bars) {
    if (bars >= 4) return 'Excellent';
    if (bars >= 3) return 'Good';
    if (bars >= 2) return 'Fair';
    return 'Poor';
}

function displayMockPeers() {
    const mockPeers = [
        {
            peer_id: '1',
            user_tag: 'alice_trader',
            wallet_address: '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU',
            discovery_method: 'WiFi',
            signal_strength: -45,
            verified: true,
            discovered_at: new Date(Date.now() - 120000).toISOString()
        },
        {
            peer_id: '2',
            user_tag: 'bob_crypto',
            wallet_address: '9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM',
            discovery_method: 'Bluetooth',
            signal_strength: -65,
            verified: true,
            discovered_at: new Date(Date.now() - 60000).toISOString()
        }
    ];
    displayDiscoveredPeers(mockPeers);
}

// Task 23.3: Transfer Request Component
function selectPeerForTransfer(peer) {
    proximityState.selectedPeer = peer;
    
    // Show transfer form
    document.getElementById('transferRequestForm').classList.remove('hidden');
    
    // Populate peer info
    document.getElementById('selectedPeerInfo').innerHTML = `
        <div class="selected-peer-header">
            <span class="selected-peer-tag">${peer.user_tag}</span>
            ${peer.verified ? '<span class="peer-badge verified">✓ Verified</span>' : ''}
        </div>
        <div class="selected-peer-wallet">${peer.wallet_address}</div>
    `;
    
    // Reset form
    document.getElementById('transferAsset').value = '';
    document.getElementById('transferAmount').value = '';
    document.getElementById('confirmTransferBtn').disabled = true;
    document.getElementById('transferFormError').textContent = '';
    
    // Scroll to form
    document.getElementById('transferRequestForm').scrollIntoView({ behavior: 'smooth' });
}

function closeTransferForm() {
    document.getElementById('transferRequestForm').classList.add('hidden');
    proximityState.selectedPeer = null;
}

function updateTransferFees() {
    const asset = document.getElementById('transferAsset').value;
    const amount = parseFloat(document.getElementById('transferAmount').value);
    
    if (!asset || !amount || amount <= 0) {
        document.getElementById('confirmTransferBtn').disabled = true;
        return;
    }
    
    // Network fee estimate (Solana base fee for SOL, slightly higher for SPL tokens)
    const networkFee = asset === 'SOL' ? 0.000005 : 0.00001;
    const total = amount + networkFee;
    
    document.getElementById('networkFeeEstimate').textContent = `${networkFee} ${asset}`;
    document.getElementById('totalToSend').textContent = `${total.toFixed(6)} ${asset}`;
    
    // Show the real available balance of the sender wallet for the selected asset.
    updateTransferBalance(asset);
    
    document.getElementById('confirmTransferBtn').disabled = false;
}

// Fetch and display the sender wallet's real available balance for the asset.
async function updateTransferBalance(asset) {
    const balanceEl = document.getElementById('transferBalance');
    if (!balanceEl) return;
    const senderWallet = state.connectedWallet || DEMO_SENDER_WALLET;
    balanceEl.textContent = `Available: … ${asset}`;
    try {
        const resp = await fetch(`${API_BASE_URL}/api/wallets/${senderWallet}/portfolio`);
        if (!resp.ok) throw new Error('balance unavailable');
        const json = await resp.json();
        const assets = (json.data && json.data.assets) || [];
        const match = assets.find(a => a.token_symbol === asset);
        if (match) {
            const amt = parseFloat(match.amount);
            balanceEl.textContent = `Available: ${amt.toLocaleString(undefined, { maximumFractionDigits: 6 })} ${asset}`;
        } else {
            balanceEl.textContent = `Available: 0 ${asset}`;
        }
    } catch (e) {
        balanceEl.textContent = `Available: -- ${asset}`;
    }
}

async function confirmTransfer() {
    const asset = document.getElementById('transferAsset').value;
    const amount = parseFloat(document.getElementById('transferAmount').value);
    const errorDiv = document.getElementById('transferFormError');
    
    if (!proximityState.selectedPeer) {
        errorDiv.textContent = 'No peer selected';
        return;
    }
    
    if (!asset || !amount || amount <= 0) {
        errorDiv.textContent = 'Please enter a valid amount';
        return;
    }
    
    if (!confirm(`Send ${amount} ${asset} to ${proximityState.selectedPeer.user_tag}?`)) {
        return;
    }
    
    showLoading();
    errorDiv.textContent = '';
    
    try {
        // The on-chain sender is the backend's funded demo wallet. Use the
        // connected wallet if the user linked one, otherwise fall back to the
        // demo sender wallet so a proximity transfer works without first
        // connecting a wallet on the dashboard.
        const senderWallet = state.connectedWallet || DEMO_SENDER_WALLET;
        const response = await fetch(`${API_BASE_URL}/api/proximity/transfers`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                sender_user_id: DEMO_USER_ID,
                sender_wallet: senderWallet,
                // Use the peer's real user account id (UUID) broadcast via
                // discovery, not the device-level peer_id, so the backend can
                // persist the transfer against a valid users(id).
                recipient_user_id: proximityState.selectedPeer.user_id || proximityState.selectedPeer.peer_id,
                recipient_wallet: proximityState.selectedPeer.wallet_address,
                asset: asset,
                amount: amount.toString()
            })
        });
        
        if (!response.ok) {
            const error = await response.json().catch(() => ({}));
            throw new Error(error.message || error.error || 'Failed to create transfer');
        }
        
        showToast('Transfer request sent!', 'success');
        closeTransferForm();
        loadProximityHistory();
        
    } catch (error) {
        console.error('Error creating transfer:', error);
        const msg = error.message || 'Failed to create transfer';
        errorDiv.textContent = msg;
        showToast(msg, 'error');
    } finally {
        hideLoading();
    }
}

// Task 23.4: Transfer Notification Component
function showIncomingTransferNotification(transfer) {
    proximityState.incomingTransfer = transfer;
    
    // Populate notification
    document.getElementById('transferNotificationContent').innerHTML = `
        <div class="notification-detail">
            <span class="notification-label">From:</span>
            <span class="notification-value">${transfer.sender_user_tag}</span>
        </div>
        <div class="notification-detail">
            <span class="notification-label">Asset:</span>
            <span class="notification-value">${transfer.asset}</span>
        </div>
        <div class="notification-detail">
            <span class="notification-label">Amount:</span>
            <span class="notification-value highlight">${transfer.amount} ${transfer.asset}</span>
        </div>
    `;
    
    // Show notification
    document.getElementById('incomingTransferNotification').classList.remove('hidden');
    
    // Start expiry timer
    startTransferNotificationTimer(transfer.expires_at);
    
    // Scroll to notification
    document.getElementById('incomingTransferNotification').scrollIntoView({ behavior: 'smooth' });
}

function startTransferNotificationTimer(expiresAt) {
    const updateTimer = () => {
        const now = new Date();
        const expiry = new Date(expiresAt);
        const remaining = Math.floor((expiry - now) / 1000);
        
        if (remaining <= 0) {
            document.getElementById('notificationTimer').textContent = 'Expired';
            stopTransferNotificationTimer();
            hideIncomingTransferNotification();
            showToast('Transfer request expired', 'warning');
        } else {
            document.getElementById('notificationTimer').textContent = `Expires in ${remaining}s`;
        }
    };
    
    updateTimer();
    proximityState.transferNotificationTimer = setInterval(updateTimer, 1000);
}

function stopTransferNotificationTimer() {
    if (proximityState.transferNotificationTimer) {
        clearInterval(proximityState.transferNotificationTimer);
        proximityState.transferNotificationTimer = null;
    }
}

function hideIncomingTransferNotification() {
    document.getElementById('incomingTransferNotification').classList.add('hidden');
    proximityState.incomingTransfer = null;
    stopTransferNotificationTimer();
}

async function acceptTransfer() {
    if (!proximityState.incomingTransfer) return;
    
    showLoading();
    
    try {
        const response = await fetch(
            `${API_BASE_URL}/api/proximity/transfers/${proximityState.incomingTransfer.transfer_id}/accept`,
            { method: 'POST' }
        );
        
        const data = await response.json().catch(() => ({}));
        if (!response.ok) {
            throw new Error(data.message || data.error || 'Failed to accept transfer');
        }
        
        // The backend either settled the transfer on-chain immediately
        // (transaction_hash present) or, if the blockchain was unreachable,
        // queued it for automatic settlement when connectivity returns.
        if (data.transaction_hash) {
            const sig = data.transaction_hash;
            showToast(`Transfer settled on-chain! Tx: ${sig.substring(0, 8)}...`, 'success');
        } else {
            showToast('Offline: transfer queued — it will sync to the blockchain when back online', 'warning');
        }
        hideIncomingTransferNotification();
        loadProximityHistory();
        
    } catch (error) {
        console.error('Error accepting transfer:', error);
        showToast(`Failed to accept transfer: ${error.message}`, 'error');
    } finally {
        hideLoading();
    }
}

async function rejectTransfer() {
    if (!proximityState.incomingTransfer) return;
    
    showLoading();
    
    try {
        const response = await fetch(
            `${API_BASE_URL}/api/proximity/transfers/${proximityState.incomingTransfer.transfer_id}/reject`,
            {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ reason: 'User declined' })
            }
        );
        
        if (!response.ok) {
            throw new Error('Failed to reject transfer');
        }
        
        showToast('Transfer rejected', 'success');
        hideIncomingTransferNotification();
        
    } catch (error) {
        console.error('Error rejecting transfer:', error);
        showToast('Failed to reject transfer', 'error');
    } finally {
        hideLoading();
    }
}

// Task 23.5: Transfer History Component
async function loadProximityHistory() {
    if (!state.connectedWallet) {
        document.getElementById('proximityHistoryList').innerHTML = 
            '<p class="empty-state">Connect your wallet to view history</p>';
        return;
    }
    
    const dateFilter = document.getElementById('historyDateFilter').value;
    const assetFilter = document.getElementById('historyAssetFilter').value;
    const typeFilter = document.getElementById('historyTypeFilter').value;
    
    try {
        const params = new URLSearchParams({
            user_id: DEMO_USER_ID,
            limit: '50',
            offset: '0'
        });
        
        // Route through apiFetch for the uniform 10s timeout + error path (Req 2.8).
        const data = await apiFetch(`/api/proximity/transfers/history?${params}`);
        let transfers = data.transfers || [];
        
        // Apply filters
        transfers = applyHistoryFilters(transfers, dateFilter, assetFilter, typeFilter);
        
        displayProximityHistory(transfers);
        
    } catch (error) {
        console.error('Error loading history:', error);
        // Mock history is only sanctioned when the backend was unreachable at
        // open (Req 2.5), always paired with the "data not live" banner. While
        // reachable, a failed request surfaces a visible error and never
        // silently substitutes mock data (Req 2.3/2.4/2.7).
        if (typeof state !== 'undefined' && state.backendReachable === false) {
            if (typeof showDataNotLiveBanner === 'function') showDataNotLiveBanner();
            displayMockProximityHistory();
        } else {
            const container = document.getElementById('proximityHistoryList');
            if (container) {
                container.innerHTML = `<p class="error-state">⚠ Failed to load transfer history: ${error.message}</p>`;
            }
            if (typeof showError === 'function') {
                showError(`Failed to load transfer history: ${error.message}`);
            }
        }
    }
}

function applyHistoryFilters(transfers, dateFilter, assetFilter, typeFilter) {
    let filtered = [...transfers];
    
    // Date filter
    if (dateFilter !== 'all') {
        const now = new Date();
        const cutoff = new Date();
        
        if (dateFilter === 'today') {
            cutoff.setHours(0, 0, 0, 0);
        } else if (dateFilter === 'week') {
            cutoff.setDate(now.getDate() - 7);
        } else if (dateFilter === 'month') {
            cutoff.setMonth(now.getMonth() - 1);
        }
        
        filtered = filtered.filter(t => new Date(t.created_at) >= cutoff);
    }
    
    // Asset filter
    if (assetFilter !== 'all') {
        filtered = filtered.filter(t => t.asset === assetFilter);
    }
    
    // Type filter (direct vs P2P)
    if (typeFilter !== 'all') {
        filtered = filtered.filter(t => {
            const isDirect = !t.is_p2p_exchange;
            return typeFilter === 'direct' ? isDirect : !isDirect;
        });
    }
    
    return filtered;
}

function displayProximityHistory(transfers) {
    const container = document.getElementById('proximityHistoryList');
    
    if (transfers.length === 0) {
        container.innerHTML = '<p class="empty-state">No transfer history</p>';
        return;
    }
    
    container.innerHTML = '';
    
    transfers.forEach(transfer => {
        const item = document.createElement('div');
        const isSent = transfer.sender_user_id === DEMO_USER_ID;
        const statusClass = transfer.status.toLowerCase();
        
        item.className = `history-transfer-item ${isSent ? 'sent' : 'received'} ${statusClass}`;
        
        item.innerHTML = `
            <div class="transfer-history-info">
                <div class="transfer-history-header">
                    <span class="transfer-direction ${isSent ? 'sent' : 'received'}">
                        ${isSent ? '→ Sent' : '← Received'}
                    </span>
                    <span class="transfer-peer-tag">
                        ${isSent ? 'To' : 'From'}: ${isSent ? 'Recipient' : 'Sender'}
                    </span>
                </div>
                <div class="transfer-history-details">
                    <span>${new Date(transfer.created_at).toLocaleString()}</span>
                    ${transfer.transaction_hash ? 
                        `<span>Tx: ${transfer.transaction_hash.substring(0, 8)}...</span>` : 
                        ''}
                </div>
            </div>
            <div class="transfer-amount-display">
                <div class="transfer-amount-value">${parseFloat(transfer.amount).toFixed(6)}</div>
                <div class="transfer-asset">${transfer.asset}</div>
                <span class="transfer-status-badge ${statusClass}">${transfer.status}</span>
                ${transfer.status === 'PendingSettlement' ?
                    `<span class="transfer-sync-hint" title="Recorded offline; will settle on-chain when connectivity returns">⏳ Will sync when online</span>` :
                    ''}
                ${transfer.status === 'Completed' ? 
                    `<button class="receipt-download-btn" onclick="downloadReceipt('${transfer.id}')">
                        Download Receipt
                    </button>` : 
                    ''}
            </div>
        `;
        
        container.appendChild(item);
    });
}

function displayMockProximityHistory() {
    const mockTransfers = [
        {
            id: '1',
            sender_user_id: DEMO_USER_ID,
            recipient_user_id: '2',
            asset: 'SOL',
            amount: '1.5',
            status: 'Completed',
            transaction_hash: '5j7s8k9d...',
            created_at: new Date(Date.now() - 3600000).toISOString(),
            is_p2p_exchange: false
        },
        {
            id: '2',
            sender_user_id: '3',
            recipient_user_id: DEMO_USER_ID,
            asset: 'USDC',
            amount: '50.00',
            status: 'Completed',
            transaction_hash: '3h4j5k6l...',
            created_at: new Date(Date.now() - 7200000).toISOString(),
            is_p2p_exchange: false
        }
    ];
    displayProximityHistory(mockTransfers);
}

async function downloadReceipt(transferId) {
    showLoading();
    
    try {
        // Mock receipt download
        showToast('Receipt downloaded (demo mode)', 'success');
        
    } catch (error) {
        console.error('Error downloading receipt:', error);
        showToast('Failed to download receipt', 'error');
    } finally {
        hideLoading();
    }
}

// WebSocket for real-time proximity events
function initializeProximityWebSocket() {
    if (proximityState.proximityWebSocket) {
        proximityState.proximityWebSocket.close();
    }
    
    try {
        const wsUrl = API_BASE_URL.replace('http', 'ws') + '/api/proximity/events';
        proximityState.proximityWebSocket = new WebSocket(wsUrl);
        
        proximityState.proximityWebSocket.onopen = () => {
            console.log('Proximity WebSocket connected');
        };
        
        proximityState.proximityWebSocket.onmessage = (event) => {
            try {
                const data = JSON.parse(event.data);
                handleProximityEvent(data);
            } catch (error) {
                console.error('Error parsing proximity WebSocket message:', error);
            }
        };
        
        proximityState.proximityWebSocket.onerror = (error) => {
            console.error('Proximity WebSocket error:', error);
        };
        
        proximityState.proximityWebSocket.onclose = () => {
            console.log('Proximity WebSocket disconnected');
        };
        
    } catch (error) {
        console.error('Error initializing proximity WebSocket:', error);
    }
}

function handleProximityEvent(event) {
    switch (event.type) {
        case 'peer_discovered':
            refreshPeers();
            showToast(`New peer discovered: ${event.user_tag}`, 'success');
            break;
            
        case 'peer_removed':
            refreshPeers();
            break;
            
        case 'transfer_request_received':
            showIncomingTransferNotification(event);
            break;
            
        case 'transfer_accepted':
            showToast('Transfer accepted by recipient', 'success');
            loadProximityHistory();
            break;
            
        case 'transfer_rejected':
            showToast('Transfer rejected by recipient', 'warning');
            loadProximityHistory();
            break;
            
        case 'transfer_completed':
            showToast(`Transfer completed! Tx: ${event.transaction_hash.substring(0, 8)}...`, 'success');
            loadProximityHistory();
            break;
            
        case 'transfer_failed':
            showToast(`Transfer failed: ${event.reason}`, 'error');
            loadProximityHistory();
            break;
            
        default:
            console.log('Unknown proximity event type:', event.type);
    }
}
